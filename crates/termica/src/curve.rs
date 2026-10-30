//! Editor de la curva escalonada del EC: 7 velocidades y 6 temperaturas de cambio.
//! Los puntos se arrastran: horizontal = temperatura, vertical = velocidad.

use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;

use gtk::cairo;
use gtk::gdk;
use gtk::prelude::*;
use termica_core::{Curve, LAST_SPEED_MIN, SPEED_MAX};

const T_MIN: f64 = 30.0;
const T_MAX: f64 = 100.0;
const PAD_L: f64 = 46.0;
const PAD_R: f64 = 18.0;
const PAD_T: f64 = 16.0;
const PAD_B: f64 = 30.0;

type Callback = Box<dyn Fn()>;

struct State {
    temps: Vec<u32>,
    speeds: Vec<u32>,
    current: Option<i64>,
    /// Punto bajo el cursor: -1 = velocidad base, 0..5 = umbrales.
    hover: Option<i32>,
    drag: Option<(i32, f64, f64)>,
}

#[derive(Clone)]
pub struct CurveEditor {
    pub area: gtk::DrawingArea,
    st: Rc<RefCell<State>>,
    on_change: Rc<RefCell<Option<Callback>>>,
}

struct Geom {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Geom {
    fn new(w: f64, h: f64) -> Self {
        Geom { x0: PAD_L, y0: PAD_T, x1: w - PAD_R, y1: h - PAD_B }
    }
    fn tx(&self, t: f64) -> f64 {
        self.x0 + (t - T_MIN) / (T_MAX - T_MIN) * (self.x1 - self.x0)
    }
    fn sy(&self, s: f64) -> f64 {
        self.y1 - s / SPEED_MAX as f64 * (self.y1 - self.y0)
    }
    fn inv_t(&self, x: f64) -> f64 {
        T_MIN + (x - self.x0) / (self.x1 - self.x0).max(1.0) * (T_MAX - T_MIN)
    }
    fn inv_s(&self, y: f64) -> f64 {
        (self.y1 - y) / (self.y1 - self.y0).max(1.0) * SPEED_MAX as f64
    }
}

impl State {
    fn points(&self, g: &Geom) -> Vec<(i32, f64, f64)> {
        let mut pts = vec![(-1, g.tx(T_MIN), g.sy(self.speeds[0] as f64))];
        for (i, &t) in self.temps.iter().enumerate() {
            pts.push((i as i32, g.tx(t as f64), g.sy(self.speeds[i + 1] as f64)));
        }
        pts
    }

    fn nearest(&self, g: &Geom, x: f64, y: f64) -> Option<i32> {
        self.points(g)
            .into_iter()
            .map(|(i, px, py)| (i, (px - x).powi(2) + (py - y).powi(2)))
            .filter(|&(_, d)| d < 18.0 * 18.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
}

impl CurveEditor {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder().content_height(280).hexpand(true).build();
        let st = Rc::new(RefCell::new(State {
            temps: vec![60, 70, 76, 82, 86, 90],
            speeds: vec![0, 30, 38, 50, 65, 85, 110],
            current: None,
            hover: None,
            drag: None,
        }));
        let ed = CurveEditor { area: area.clone(), st, on_change: Rc::new(RefCell::new(None)) };

        let e = ed.clone();
        area.set_draw_func(move |area, cr, w, h| e.draw(area, cr, w as f64, h as f64));

        let drag = gtk::GestureDrag::new();
        let e = ed.clone();
        drag.connect_drag_begin(move |g, x, y| {
            let geom = e.geom();
            let idx = e.st.borrow().nearest(&geom, x, y);
            match idx {
                Some(i) => {
                    e.st.borrow_mut().drag = Some((i, x, y));
                    e.area.set_cursor_from_name(Some("grabbing"));
                }
                None => {
                    g.set_state(gtk::EventSequenceState::Denied);
                }
            }
        });
        let e = ed.clone();
        drag.connect_drag_update(move |_, dx, dy| e.drag_to(dx, dy));
        let e = ed.clone();
        drag.connect_drag_end(move |_, _, _| {
            let hover = {
                let mut st = e.st.borrow_mut();
                st.drag = None;
                st.hover
            };
            e.area.set_cursor_from_name(hover.map(|_| "grab"));
        });
        area.add_controller(drag);

        let motion = gtk::EventControllerMotion::new();
        let e = ed.clone();
        motion.connect_motion(move |_, x, y| {
            if e.st.borrow().drag.is_none() {
                let geom = e.geom();
                let idx = e.st.borrow().nearest(&geom, x, y);
                e.set_hover(idx);
            }
        });
        let e = ed.clone();
        motion.connect_leave(move |_| e.set_hover(None));
        area.add_controller(motion);

        let sm = adw::StyleManager::default();
        let a = area.clone();
        sm.connect_dark_notify(move |_| a.queue_draw());
        let a = area.clone();
        sm.connect_accent_color_rgba_notify(move |_| a.queue_draw());
        ed
    }

    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_curve(&self, c: &Curve) {
        let mut st = self.st.borrow_mut();
        st.temps = c.temps.clone();
        st.speeds = c.speeds.clone();
        drop(st);
        self.area.queue_draw();
    }

    pub fn curve(&self) -> Curve {
        let st = self.st.borrow();
        Curve { temps: st.temps.clone(), speeds: st.speeds.clone() }
    }

    pub fn set_current(&self, t: Option<i64>) {
        if self.st.borrow().current != t {
            self.st.borrow_mut().current = t;
            self.area.queue_draw();
        }
    }

    fn geom(&self) -> Geom {
        Geom::new(self.area.width() as f64, self.area.height() as f64)
    }

    fn set_hover(&self, idx: Option<i32>) {
        if self.st.borrow().hover != idx {
            self.st.borrow_mut().hover = idx;
            self.area.set_cursor_from_name(idx.map(|_| "grab"));
            self.area.queue_draw();
        }
    }

    fn drag_to(&self, dx: f64, dy: f64) {
        let g = self.geom();
        {
            let mut st = self.st.borrow_mut();
            let Some((idx, sx, sy)) = st.drag else { return };
            let (x, y) = (sx + dx, sy + dy);
            let si = (idx + 1) as usize; // velocidad que controla este punto
            let mut lo_s = if si > 0 { st.speeds[si - 1] } else { 0 };
            let hi_s = if si < 6 { st.speeds[si + 1] } else { SPEED_MAX };
            if si == 6 {
                lo_s = lo_s.max(LAST_SPEED_MIN);
            }
            let s = g.inv_s(y).round().max(0.0) as u32;
            st.speeds[si] = s.clamp(lo_s, hi_s);
            if idx >= 0 {
                let i = idx as usize;
                let lo_t = if i > 0 { st.temps[i - 1] + 1 } else { T_MIN as u32 };
                let hi_t = if i < 5 { st.temps[i + 1] - 1 } else { T_MAX as u32 };
                let t = g.inv_t(x).round().max(0.0) as u32;
                st.temps[i] = t.clamp(lo_t, hi_t);
            }
            st.hover = Some(idx);
        }
        self.area.queue_draw();
        if let Some(f) = self.on_change.borrow().as_ref() {
            f();
        }
    }

    /// Texto centrado verticalmente en `y`; `align` 0 = izquierda, 0.5 = centro, 1 = derecha.
    fn text(&self, cr: &cairo::Context, text: &str, (x, y): (f64, f64), (color, alpha): (&gdk::RGBA, f64), align: f64) {
        let layout = self.area.create_pango_layout(Some(text));
        let (w, h) = layout.pixel_size();
        cr.set_source_rgba(color.red() as f64, color.green() as f64, color.blue() as f64, alpha);
        cr.move_to(x - w as f64 * align, y - h as f64 / 2.0);
        pangocairo::functions::show_layout(cr, &layout);
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &cairo::Context, w: f64, h: f64) {
        let st = self.st.borrow();
        let g = Geom::new(w, h);
        let fg = area.color();
        let (fr, fgr, fb) = (fg.red() as f64, fg.green() as f64, fg.blue() as f64);
        let sm = adw::StyleManager::default();
        let accent = sm.accent_color_rgba();
        let (ar, ag, ab) = (accent.red() as f64, accent.green() as f64, accent.blue() as f64);
        let bg = if sm.is_dark() { (0.118, 0.118, 0.118) } else { (1.0, 1.0, 1.0) };

        // rejilla y ejes
        cr.set_line_width(1.0);
        for s in (0..=SPEED_MAX).step_by(25) {
            let y = g.sy(s as f64);
            cr.set_source_rgba(fr, fgr, fb, 0.10);
            cr.move_to(g.x0, y);
            cr.line_to(g.x1, y);
            let _ = cr.stroke();
            self.text(cr, &format!("{s}%"), (g.x0 - 8.0, y), (&fg, 0.55), 1.0);
        }
        for t in (T_MIN as u32..=T_MAX as u32).step_by(10) {
            let x = g.tx(t as f64);
            cr.set_source_rgba(fr, fgr, fb, 0.06);
            cr.move_to(x, g.y0);
            cr.line_to(x, g.y1);
            let _ = cr.stroke();
            self.text(cr, &format!("{t}°"), (x, g.y1 + 16.0), (&fg, 0.55), 0.5);
        }

        // zona peligrosa (≥ 90 °C)
        cr.set_source_rgba(0.88, 0.11, 0.14, 0.07);
        cr.rectangle(g.tx(90.0), g.y0, g.x1 - g.tx(90.0), g.y1 - g.y0);
        let _ = cr.fill();

        // curva escalonada
        let path = |cr: &cairo::Context| {
            cr.move_to(g.x0, g.sy(st.speeds[0] as f64));
            for (i, &t) in st.temps.iter().enumerate() {
                let x = g.tx(t as f64);
                cr.line_to(x, g.sy(st.speeds[i] as f64));
                cr.line_to(x, g.sy(st.speeds[i + 1] as f64));
            }
            cr.line_to(g.x1, g.sy(st.speeds[6] as f64));
        };
        path(cr);
        cr.line_to(g.x1, g.y1);
        cr.line_to(g.x0, g.y1);
        cr.close_path();
        cr.set_source_rgba(ar, ag, ab, 0.16);
        let _ = cr.fill();
        path(cr);
        cr.set_source_rgba(ar, ag, ab, 1.0);
        cr.set_line_width(2.5);
        cr.set_line_join(cairo::LineJoin::Round);
        let _ = cr.stroke();

        // temperatura actual
        if let Some(cur) = st.current
            && (T_MIN as i64..=T_MAX as i64).contains(&cur)
        {
            let x = g.tx(cur as f64);
            cr.set_source_rgba(fr, fgr, fb, 0.65);
            cr.set_dash(&[4.0, 4.0], 0.0);
            cr.set_line_width(1.5);
            cr.move_to(x, g.y0);
            cr.line_to(x, g.y1);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
            self.text(cr, &format!("Ahora {cur}°"), ((x + 6.0).min(g.x1 - 70.0), g.y0 + 8.0), (&fg, 0.8), 0.0);
        }

        // puntos
        let points = st.points(&g);
        for &(i, px, py) in &points {
            let r = if Some(i) == st.hover { 8.0 } else { 6.0 };
            cr.arc(px, py, r + 2.5, 0.0, 2.0 * PI);
            cr.set_source_rgb(bg.0, bg.1, bg.2);
            let _ = cr.fill();
            cr.arc(px, py, r, 0.0, 2.0 * PI);
            cr.set_source_rgb(ar, ag, ab);
            let _ = cr.fill();
        }

        // etiqueta del punto activo
        if let Some(i) = st.hover
            && let Some(&(_, px, py)) = points.iter().find(|p| p.0 == i)
        {
            let label = if i < 0 {
                format!("Base: {}%", st.speeds[0])
            } else {
                format!("Desde {}° → {}%", st.temps[i as usize], st.speeds[i as usize + 1])
            };
            let layout = self.area.create_pango_layout(Some(&label));
            let (lw, lh) = layout.pixel_size();
            let (lw, lh) = (lw as f64, lh as f64);
            let bx = (px - lw / 2.0 - 8.0).max(g.x0).min(g.x1 - lw - 16.0);
            let by = if py - lh - 22.0 > 2.0 { py - lh - 22.0 } else { py + 14.0 };
            cr.set_source_rgba(fr, fgr, fb, 0.92);
            rounded(cr, bx, by, lw + 16.0, lh + 8.0, 6.0);
            let _ = cr.fill();
            cr.set_source_rgb(bg.0, bg.1, bg.2);
            cr.move_to(bx + 8.0, by + 4.0);
            pangocairo::functions::show_layout(cr, &layout);
        }
    }
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

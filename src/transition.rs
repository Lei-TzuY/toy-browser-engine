// ============================================================
//  transition.rs  —  CSS Transitions & Value Interpolation
// ============================================================
//
//  Handles:
//   • Cubic Bezier timing functions (linear, ease, ease-in, ease-out, ease-in-out, cubic-bezier)
//   • Interpolation between CSS Values:
//       - Colors (RGBA linear interpolation)
//       - Lengths (px, em, %, etc.)
//       - Numbers (opacity, z-index, line-height, etc.)
//       - Transforms (translate_x, translate_y, scale)
//       - BoxShadows (offset_x, offset_y, blur_radius, color)
//   • TransitionManager:
//       - Tracking active transitions per ElementId
//       - Smooth retargeting when an active transition changes destination
//       - Overriding computed specified_values with current interpolated values

use std::collections::HashMap;

use crate::css::parser::{
    parse_time_to_ms, parse_timing_func, BoxShadow, Color, TimingFunction, Transform,
    TransitionSpec, Value,
};
use crate::dom::ElementId;
use crate::style::{PropertyMap, StyledNode};

/// Interpolate between two CSS values by fraction `t` in [0.0, 1.0].
pub fn interpolate(from: &Value, to: &Value, t: f32) -> Value {
    let t = t.clamp(0.0, 1.0);

    match (from, to) {
        (Value::Color(c1), Value::Color(c2)) => {
            let r = (c1.r as f32 + (c2.r as f32 - c1.r as f32) * t)
                .round()
                .clamp(0.0, 255.0) as u8;
            let g = (c1.g as f32 + (c2.g as f32 - c1.g as f32) * t)
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = (c1.b as f32 + (c2.b as f32 - c1.b as f32) * t)
                .round()
                .clamp(0.0, 255.0) as u8;
            let a = (c1.a as f32 + (c2.a as f32 - c1.a as f32) * t)
                .round()
                .clamp(0.0, 255.0) as u8;
            Value::Color(Color::rgba(r, g, b, a))
        }
        (Value::Length(v1, u1), Value::Length(v2, u2)) if u1 == u2 => {
            Value::Length(v1 + (v2 - v1) * t, u1.clone())
        }
        (Value::Number(n1), Value::Number(n2)) => Value::Number(n1 + (n2 - n1) * t),
        (Value::Transform(t1), Value::Transform(t2)) => Value::Transform(Transform {
            translate_x: t1.translate_x + (t2.translate_x - t1.translate_x) * t,
            translate_y: t1.translate_y + (t2.translate_y - t1.translate_y) * t,
            scale: t1.scale + (t2.scale - t1.scale) * t,
        }),
        (Value::BoxShadow(s1), Value::BoxShadow(s2)) => {
            let color = match interpolate(&Value::Color(s1.color), &Value::Color(s2.color), t) {
                Value::Color(c) => c,
                _ => s2.color,
            };
            Value::BoxShadow(BoxShadow {
                offset_x: s1.offset_x + (s2.offset_x - s1.offset_x) * t,
                offset_y: s1.offset_y + (s2.offset_y - s1.offset_y) * t,
                blur_radius: s1.blur_radius + (s2.blur_radius - s1.blur_radius) * t,
                color,
            })
        }
        _ => {
            if t < 1.0 {
                from.clone()
            } else {
                to.clone()
            }
        }
    }
}

/// An active in-flight transition on an element property.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTransition {
    pub property: String,
    pub from: Value,
    pub to: Value,
    pub start_time_ms: f64,
    pub duration_ms: f64,
    pub delay_ms: f64,
    pub timing_function: TimingFunction,
}

impl ActiveTransition {
    /// Calculate the current interpolated value at `now_ms`.
    pub fn current_value(&self, now_ms: f64) -> Value {
        let elapsed = (now_ms - (self.start_time_ms + self.delay_ms)) as f32;
        if elapsed <= 0.0 {
            return self.from.clone();
        }
        let duration = (self.duration_ms as f32).max(1.0);
        let progress = (elapsed / duration).clamp(0.0, 1.0);
        let eased = self.timing_function.evaluate(progress);
        interpolate(&self.from, &self.to, eased)
    }

    /// True if the transition has finished.
    pub fn is_finished(&self, now_ms: f64) -> bool {
        now_ms >= self.start_time_ms + self.delay_ms + self.duration_ms
    }
}

/// Manages CSS transitions across all elements in a Document.
#[derive(Debug, Default, Clone)]
pub struct TransitionManager {
    pub active: HashMap<ElementId, HashMap<String, ActiveTransition>>,
    pub previous_styles: HashMap<ElementId, PropertyMap>,
}

impl TransitionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.active.clear();
        self.previous_styles.clear();
    }

    pub fn has_active(&self) -> bool {
        self.active.values().any(|m| !m.is_empty())
    }

    pub fn active_count(&self) -> usize {
        self.active.values().map(|m| m.len()).sum()
    }

    /// Walk the styled node tree, detect style changes, initiate transitions, and apply
    /// active transition values into `specified_values`.
    pub fn update_and_apply(&mut self, root: &mut StyledNode<'_>, now_ms: f64) {
        self.walk_and_apply(root, now_ms);
    }

    fn walk_and_apply(&mut self, node: &mut StyledNode<'_>, now_ms: f64) {
        if let Some(element) = node.node.as_element() {
            let elem_id = element.element_id();
            self.process_element(elem_id, &mut node.specified_values, now_ms);
        }
        for child in &mut node.children {
            self.walk_and_apply(child, now_ms);
        }
    }

    fn process_element(&mut self, elem_id: ElementId, specified: &mut PropertyMap, now_ms: f64) {
        let mut specs = Vec::new();
        if let Some(Value::Transition(trans_specs)) = specified.get("transition") {
            specs.extend(trans_specs.clone());
        }
        if let Some(prop_val) = specified.get("transition-property") {
            let dur_ms = match specified.get("transition-duration") {
                Some(Value::Number(n)) => *n,
                Some(Value::Length(n, _)) => *n,
                Some(Value::Keyword(s)) => parse_time_to_ms(s).unwrap_or(0.0),
                _ => 0.0,
            };
            let delay_ms = match specified.get("transition-delay") {
                Some(Value::Number(n)) => *n,
                Some(Value::Length(n, _)) => *n,
                Some(Value::Keyword(s)) => parse_time_to_ms(s).unwrap_or(0.0),
                _ => 0.0,
            };
            let timing_fn = match specified.get("transition-timing-function") {
                Some(Value::Keyword(s)) => parse_timing_func(s).unwrap_or(TimingFunction::Ease),
                _ => TimingFunction::Ease,
            };
            let prop_name = match prop_val {
                Value::Keyword(s) => s.to_ascii_lowercase(),
                _ => "all".to_string(),
            };
            specs.push(TransitionSpec {
                property: prop_name,
                duration_ms: dur_ms,
                timing_function: timing_fn,
                delay_ms,
            });
        }

        let prev_styles = self.previous_styles.get(&elem_id).cloned();

        if let Some(prev) = prev_styles {
            for (prop, new_val) in specified.iter() {
                if prop.starts_with("transition") {
                    continue;
                }
                if let Some(old_val) = prev.get(prop) {
                    if old_val != new_val {
                        let matching_spec = specs
                            .iter()
                            .find(|s| s.property == *prop || s.property == "all");
                        if let Some(spec) = matching_spec {
                            if spec.duration_ms > 0.0 {
                                let element_active = self.active.entry(elem_id).or_default();
                                let from_val = if let Some(existing) = element_active.get(prop) {
                                    existing.current_value(now_ms)
                                } else {
                                    old_val.clone()
                                };
                                element_active.insert(
                                    prop.clone(),
                                    ActiveTransition {
                                        property: prop.clone(),
                                        from: from_val,
                                        to: new_val.clone(),
                                        start_time_ms: now_ms,
                                        duration_ms: spec.duration_ms as f64,
                                        delay_ms: spec.delay_ms as f64,
                                        timing_function: spec.timing_function.clone(),
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }

        self.previous_styles.insert(elem_id, specified.clone());

        if let Some(element_active) = self.active.get_mut(&elem_id) {
            element_active.retain(|prop, trans| {
                if trans.is_finished(now_ms) {
                    false
                } else {
                    let current = trans.current_value(now_ms);
                    specified.insert(prop.clone(), current);
                    true
                }
            });
        }
    }
}

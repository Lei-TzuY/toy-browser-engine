// ============================================================
//  animation.rs  —  CSS Keyframe Animations Engine
// ============================================================
//
//  Handles:
//   • @keyframes rule evaluation and multi-step keyframe interpolation
//   • Animation timing, iterations (finite & infinite), delays, directions (normal, reverse, alternate)
//   • Fill modes (none, forwards, backwards, both)
//   • AnimationManager: tracking active animations and applying animated overrides to StyledNode

use std::collections::HashMap;

use crate::css::parser::{
    parse_animation_direction, parse_animation_fill_mode, parse_animation_iteration_count,
    parse_time_to_ms, parse_timing_func, AnimationDirection, AnimationFillMode,
    AnimationIterationCount, AnimationSpec, KeyframeRule, Stylesheet, TimingFunction, Value,
};
use crate::dom::ElementId;
use crate::style::{PropertyMap, StyledNode};
use crate::transition::interpolate;

/// An active animation attached to an element.
#[derive(Debug, Clone)]
pub struct ActiveAnimation {
    pub spec: AnimationSpec,
    pub start_time_ms: f64,
}

impl ActiveAnimation {
    pub fn new(spec: AnimationSpec, start_time_ms: f64) -> Self {
        Self {
            spec,
            start_time_ms,
        }
    }

    /// Is the animation still running at `now_ms`?
    pub fn is_active(&self, now_ms: f64) -> bool {
        let elapsed = now_ms - (self.start_time_ms + self.spec.delay_ms as f64);
        if elapsed < 0.0 {
            return matches!(
                self.spec.fill_mode,
                AnimationFillMode::Backwards | AnimationFillMode::Both
            );
        }
        match self.spec.iteration_count {
            AnimationIterationCount::Infinite => true,
            AnimationIterationCount::Finite(count) => {
                let total_duration = self.spec.duration_ms as f64 * count as f64;
                if elapsed <= total_duration {
                    true
                } else {
                    matches!(
                        self.spec.fill_mode,
                        AnimationFillMode::Forwards | AnimationFillMode::Both
                    )
                }
            }
        }
    }

    /// Calculate normalized progress in [0.0, 1.0] across the keyframes at `now_ms`.
    /// Returns `None` if the animation is inactive and has no fill mode effect.
    pub fn sample_progress(&self, now_ms: f64) -> Option<f32> {
        let elapsed = now_ms - (self.start_time_ms + self.spec.delay_ms as f64);
        let duration = (self.spec.duration_ms as f64).max(1.0);

        if elapsed < 0.0 {
            if matches!(
                self.spec.fill_mode,
                AnimationFillMode::Backwards | AnimationFillMode::Both
            ) {
                return Some(self.apply_direction_at_cycle(0, 0.0));
            } else {
                return None;
            }
        }

        match self.spec.iteration_count {
            AnimationIterationCount::Infinite => {
                let cycle_index = (elapsed / duration).floor() as u64;
                let raw_cycle_progress = ((elapsed % duration) / duration) as f32;
                let dir_progress = self.apply_direction_at_cycle(cycle_index, raw_cycle_progress);
                Some(self.spec.timing_function.evaluate(dir_progress))
            }
            AnimationIterationCount::Finite(count) => {
                let total_duration = duration * count as f64;
                if elapsed >= total_duration {
                    if matches!(
                        self.spec.fill_mode,
                        AnimationFillMode::Forwards | AnimationFillMode::Both
                    ) {
                        let final_cycle = (count.ceil() as u64).saturating_sub(1);
                        let final_raw = if count.fract() == 0.0 {
                            1.0f32
                        } else {
                            count.fract()
                        };
                        let dir_progress = self.apply_direction_at_cycle(final_cycle, final_raw);
                        Some(self.spec.timing_function.evaluate(dir_progress))
                    } else {
                        None
                    }
                } else {
                    let cycle_index = (elapsed / duration).floor() as u64;
                    let raw_cycle_progress = ((elapsed % duration) / duration) as f32;
                    let dir_progress = self.apply_direction_at_cycle(cycle_index, raw_cycle_progress);
                    Some(self.spec.timing_function.evaluate(dir_progress))
                }
            }
        }
    }

    fn apply_direction_at_cycle(&self, cycle_index: u64, raw_progress: f32) -> f32 {
        let p = raw_progress.clamp(0.0, 1.0);
        match self.spec.direction {
            AnimationDirection::Normal => p,
            AnimationDirection::Reverse => 1.0 - p,
            AnimationDirection::Alternate => {
                if cycle_index % 2 == 0 {
                    p
                } else {
                    1.0 - p
                }
            }
            AnimationDirection::AlternateReverse => {
                if cycle_index % 2 == 0 {
                    1.0 - p
                } else {
                    p
                }
            }
        }
    }
}

/// Sample keyframe declarations at progress `t` in [0.0, 1.0].
pub fn sample_keyframes(rule: &KeyframeRule, t: f32) -> PropertyMap {
    let mut result = PropertyMap::new();
    if rule.steps.is_empty() {
        return result;
    }

    let t = t.clamp(0.0, 1.0);

    // Collect all properties animated by this keyframe rule
    let mut animated_properties = Vec::new();
    for step in &rule.steps {
        for decl in &step.declarations {
            if !animated_properties.contains(&decl.name) {
                animated_properties.push(decl.name.clone());
            }
        }
    }

    for prop in animated_properties {
        // Find step <= t and step >= t containing this property
        let mut prev_step: Option<(&crate::css::parser::Declaration, f32)> = None;
        let mut next_step: Option<(&crate::css::parser::Declaration, f32)> = None;

        for step in &rule.steps {
            if let Some(decl) = step.declarations.iter().find(|d| d.name == prop) {
                if step.offset <= t {
                    prev_step = Some((decl, step.offset));
                }
                if step.offset >= t && next_step.is_none() {
                    next_step = Some((decl, step.offset));
                }
            }
        }

        match (prev_step, next_step) {
            (Some((d1, o1)), Some((d2, o2))) => {
                if (o2 - o1).abs() < 1e-4 {
                    result.insert(prop, d1.value.clone());
                } else {
                    let local_t = ((t - o1) / (o2 - o1)).clamp(0.0, 1.0);
                    result.insert(prop, interpolate(&d1.value, &d2.value, local_t));
                }
            }
            (Some((d1, _)), None) => {
                result.insert(prop, d1.value.clone());
            }
            (None, Some((d2, _))) => {
                result.insert(prop, d2.value.clone());
            }
            (None, None) => {}
        }
    }

    result
}

/// Manages active CSS animations across the Document.
#[derive(Debug, Default, Clone)]
pub struct AnimationManager {
    pub active: HashMap<ElementId, Vec<ActiveAnimation>>,
}

impl AnimationManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.active.clear();
    }

    pub fn has_active(&self, now_ms: f64) -> bool {
        self.active
            .values()
            .any(|list| list.iter().any(|anim| anim.is_active(now_ms)))
    }

    pub fn update_and_apply(
        &mut self,
        root: &mut StyledNode<'_>,
        stylesheet: &Stylesheet,
        now_ms: f64,
    ) {
        self.walk_and_apply(root, stylesheet, now_ms);
    }

    fn walk_and_apply(
        &mut self,
        node: &mut StyledNode<'_>,
        stylesheet: &Stylesheet,
        now_ms: f64,
    ) {
        if let Some(element) = node.node.as_element() {
            let elem_id = element.element_id();
            self.process_element(elem_id, &mut node.specified_values, stylesheet, now_ms);
        }
        for child in &mut node.children {
            self.walk_and_apply(child, stylesheet, now_ms);
        }
    }

    fn process_element(
        &mut self,
        elem_id: ElementId,
        specified: &mut PropertyMap,
        stylesheet: &Stylesheet,
        now_ms: f64,
    ) {
        let mut specs = Vec::new();
        if let Some(Value::Animation(anim_specs)) = specified.get("animation") {
            specs.extend(anim_specs.clone());
        }
        if let Some(name_val) = specified.get("animation-name") {
            let name_str = match name_val {
                Value::Keyword(s) => s.clone(),
                _ => String::new(),
            };
            if !name_str.is_empty() && name_str != "none" {
                let dur_ms = match specified.get("animation-duration") {
                    Some(Value::Number(n)) => *n,
                    Some(Value::Length(n, _)) => *n,
                    Some(Value::Keyword(s)) => parse_time_to_ms(s).unwrap_or(0.0),
                    _ => 0.0,
                };
                let delay_ms = match specified.get("animation-delay") {
                    Some(Value::Number(n)) => *n,
                    Some(Value::Length(n, _)) => *n,
                    Some(Value::Keyword(s)) => parse_time_to_ms(s).unwrap_or(0.0),
                    _ => 0.0,
                };
                let timing_fn = match specified.get("animation-timing-function") {
                    Some(Value::Keyword(s)) => parse_timing_func(s).unwrap_or(TimingFunction::Ease),
                    _ => TimingFunction::Ease,
                };
                let iteration_count = match specified.get("animation-iteration-count") {
                    Some(Value::Keyword(s)) => {
                        parse_animation_iteration_count(s).unwrap_or_default()
                    }
                    Some(Value::Number(n)) => AnimationIterationCount::Finite(*n),
                    _ => AnimationIterationCount::default(),
                };
                let direction = match specified.get("animation-direction") {
                    Some(Value::Keyword(s)) => parse_animation_direction(s).unwrap_or_default(),
                    _ => AnimationDirection::default(),
                };
                let fill_mode = match specified.get("animation-fill-mode") {
                    Some(Value::Keyword(s)) => parse_animation_fill_mode(s).unwrap_or_default(),
                    _ => AnimationFillMode::default(),
                };
                specs.push(AnimationSpec {
                    name: name_str,
                    duration_ms: dur_ms,
                    timing_function: timing_fn,
                    delay_ms,
                    iteration_count,
                    direction,
                    fill_mode,
                });
            }
        }

        // Synchronize active animation list for this element
        let active_list = self.active.entry(elem_id).or_default();

        // Check for new specs
        for spec in &specs {
            if spec.duration_ms > 0.0 || spec.delay_ms > 0.0 {
                let already_present = active_list.iter().any(|a| a.spec.name == spec.name);
                if !already_present {
                    active_list.push(ActiveAnimation::new(spec.clone(), now_ms));
                }
            }
        }

        // Apply all active animations from active_list
        active_list.retain_mut(|anim| {
            if !anim.is_active(now_ms) {
                false
            } else {
                if let Some(keyframe_rule) = stylesheet.keyframes.get(&anim.spec.name) {
                    if let Some(progress) = anim.sample_progress(now_ms) {
                        let animated_values = sample_keyframes(keyframe_rule, progress);
                        for (k, v) in animated_values {
                            specified.insert(k, v);
                        }
                    }
                }
                true
            }
        });
    }
}

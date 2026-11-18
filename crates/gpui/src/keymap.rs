mod binding;
mod context;
mod held;

pub use binding::*;
pub use context::*;
pub(crate) use held::*;

use crate::{Action, AsKeystroke, Keystroke, SharedString, Unbind, is_no_action, is_unbind};
use collections::{HashSet, TypeIdHashMap};
use smallvec::SmallVec;

/// An opaque identifier of which version of the keymap is currently active.
/// The keymap's version is changed whenever bindings are added or removed.
#[derive(Copy, Clone, Eq, PartialEq, Default)]
pub struct KeymapVersion(usize);

/// A collection of key bindings for the user's application.
#[derive(Default)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
    binding_indices_by_action_id: TypeIdHashMap<SmallVec<[usize; 3]>>,
    disabled_binding_indices: Vec<usize>,
    held_modifiers: Vec<HeldModifier>,
    version: KeymapVersion,
}

/// Index of a binding within a keymap.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct BindingIndex(usize);

fn disabled_binding_matches_context(disabled_binding: &KeyBinding, binding: &KeyBinding) -> bool {
    if !disabled_binding
        .when_held()
        .iter()
        .all(|name| binding.when_held().contains(name))
    {
        return false;
    }
    match (
        &disabled_binding.context_predicate,
        &binding.context_predicate,
    ) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(disabled_predicate), Some(predicate)) => disabled_predicate.is_superset(predicate),
    }
}

fn binding_is_unbound(disabled_binding: &KeyBinding, binding: &KeyBinding) -> bool {
    disabled_binding.keystrokes == binding.keystrokes
        && disabled_binding
            .action()
            .as_any()
            .downcast_ref::<Unbind>()
            .is_some_and(|unbind| unbind.0.as_ref() == binding.action.name())
}

impl Keymap {
    /// Create a new keymap with the given bindings.
    pub fn new(bindings: Vec<KeyBinding>) -> Self {
        let mut this = Self::default();
        this.add_bindings(bindings);
        this
    }

    /// Get the current version of the keymap.
    pub fn version(&self) -> KeymapVersion {
        self.version
    }

    /// Add more bindings to the keymap.
    pub fn add_bindings<T: IntoIterator<Item = KeyBinding>>(&mut self, bindings: T) {
        for binding in bindings {
            if let Some(name) = binding.held_modifier() {
                self.held_modifiers.push(HeldModifier {
                    key: binding.keystrokes()[0].clone(),
                    predicate: binding.context_predicate.clone(),
                    name: name.clone(),
                });
                continue;
            }
            let action_id = binding.action().as_any().type_id();
            if is_no_action(&*binding.action) || is_unbind(&*binding.action) {
                self.disabled_binding_indices.push(self.bindings.len());
            } else {
                self.binding_indices_by_action_id
                    .entry(action_id)
                    .or_default()
                    .push(self.bindings.len());
            }
            self.bindings.push(binding);
        }

        self.version.0 += 1;
    }

    /// Reset this keymap to its initial state.
    pub fn clear(&mut self) {
        self.bindings.clear();
        self.binding_indices_by_action_id.clear();
        self.disabled_binding_indices.clear();
        self.held_modifiers.clear();
        self.version.0 += 1;
    }

    /// Iterate over all bindings, in the order they were added.
    pub fn bindings(&self) -> impl DoubleEndedIterator<Item = &KeyBinding> + ExactSizeIterator {
        self.bindings.iter()
    }

    pub(crate) fn has_held_modifiers(&self) -> bool {
        !self.held_modifiers.is_empty()
    }

    /// Cheap key-only rejection before building a context stack.
    pub(crate) fn might_activate_held_modifier(&self, input: &Keystroke) -> bool {
        self.held_modifiers
            .iter()
            .any(|declaration| input.should_match(&declaration.key))
    }

    pub(crate) fn held_modifier_for_input(
        &self,
        input: &Keystroke,
        contexts: &[KeyContext],
    ) -> Option<&HeldModifier> {
        self.held_modifiers.iter().rev().find(|declaration| {
            input.should_match(&declaration.key)
                && declaration
                    .predicate
                    .as_ref()
                    .is_none_or(|predicate| predicate.eval(contexts))
        })
    }

    /// Iterate over all bindings for the given action, in the order they were added. For display,
    /// the last binding should take precedence.
    pub fn bindings_for_action<'a>(
        &'a self,
        action: &'a dyn Action,
    ) -> impl 'a + DoubleEndedIterator<Item = &'a KeyBinding> {
        let action_id = action.type_id();
        let binding_indices = self
            .binding_indices_by_action_id
            .get(&action_id)
            .map_or(&[] as _, SmallVec::as_slice)
            .iter();

        binding_indices.filter_map(|ix| {
            let binding = &self.bindings[*ix];
            if !binding.action().partial_eq(action) {
                return None;
            }

            for disabled_ix in &self.disabled_binding_indices {
                if disabled_ix > ix {
                    let disabled_binding = &self.bindings[*disabled_ix];
                    if disabled_binding.keystrokes != binding.keystrokes {
                        continue;
                    }

                    if is_no_action(&*disabled_binding.action) {
                        if disabled_binding_matches_context(disabled_binding, binding) {
                            return None;
                        }
                    } else if is_unbind(&*disabled_binding.action)
                        && disabled_binding_matches_context(disabled_binding, binding)
                        && binding_is_unbound(disabled_binding, binding)
                    {
                        return None;
                    }
                }
            }

            Some(binding)
        })
    }

    /// Native menus cannot express transient held conditions.
    pub fn bindings_for_native_menu_action<'a>(
        &'a self,
        action: &'a dyn Action,
    ) -> impl 'a + DoubleEndedIterator<Item = &'a KeyBinding> {
        self.bindings_for_action(action)
            .filter(|binding| binding.when_held().is_empty())
    }

    fn binding_uses_held_modifier(&self, binding: &KeyBinding, contexts: &[KeyContext]) -> bool {
        binding.keystrokes().iter().any(|key| {
            self.held_modifier_for_input(key.as_keystroke(), contexts)
                .is_some()
        })
    }

    pub(crate) fn bindings_for_action_with_held<'a>(
        &'a self,
        action: &'a dyn Action,
        contexts: &'a [KeyContext],
        held: &'a [SharedString],
    ) -> impl 'a + DoubleEndedIterator<Item = &'a KeyBinding> {
        self.binding_indices_by_action_id
            .get(&action.type_id())
            .map_or(&[] as &[_], SmallVec::as_slice)
            .iter()
            .filter_map(move |&ix| {
                let binding = &self.bindings[ix];
                if !binding.action().partial_eq(action)
                    || self.binding_uses_held_modifier(binding, contexts)
                {
                    return None;
                }
                let (matches, _) = self.resolve_bindings_for_input(
                    binding.keystrokes(),
                    contexts,
                    held,
                    self.bindings().enumerate().rev(),
                    false,
                );
                matches
                    .first()
                    .is_some_and(|(first, _)| first.0 == ix)
                    .then_some(binding)
            })
    }

    /// Returns all bindings that might match the input without checking context. The bindings
    /// returned in precedence order (reverse of the order they were added to the keymap).
    pub fn all_bindings_for_input(&self, input: &[Keystroke]) -> Vec<KeyBinding> {
        self.bindings()
            .rev()
            .filter(|binding| {
                binding
                    .match_keystrokes(input)
                    .is_some_and(|pending| !pending)
            })
            .cloned()
            .collect()
    }

    /// Returns a list of bindings that match the given input, and a boolean indicating whether or
    /// not more bindings might match if the input was longer. Bindings are returned in precedence
    /// order (higher precedence first, reverse of the order they were added to the keymap).
    ///
    /// Precedence is defined by the depth in the tree (matches on the Editor take precedence over
    /// matches on the Pane, then the Workspace, etc.). Bindings with no context are treated as the
    /// same as the deepest context.
    ///
    /// In the case of multiple bindings at the same depth, the ones added to the keymap later take
    /// precedence. User bindings are added after built-in bindings so that they take precedence.
    ///
    /// If a binding has been disabled with `"x": null` it will not be returned. Disabled bindings
    /// are evaluated with the same precedence rules so you can disable a rule in a given context
    /// only. A disabled binding only suppresses bindings from sources with equal or weaker
    /// precedence: a base keymap null hides default bindings, but user bindings still apply.
    pub fn bindings_for_input(
        &self,
        input: &[impl AsKeystroke],
        context_stack: &[KeyContext],
    ) -> (SmallVec<[KeyBinding; 1]>, bool) {
        self.bindings_for_input_with_held(input, context_stack, &[])
    }

    /// Resolve bindings with the named modifiers currently held in this window.
    /// Context depth wins first, then the number of required held modifiers, then
    /// the usual reverse keymap order. Extra active names are allowed.
    pub fn bindings_for_input_with_held(
        &self,
        input: &[impl AsKeystroke],
        context_stack: &[KeyContext],
        held: &[SharedString],
    ) -> (SmallVec<[KeyBinding; 1]>, bool) {
        let (bindings, pending) = self.resolve_bindings_for_input(
            input,
            context_stack,
            held,
            self.bindings().enumerate().rev(),
            true,
        );
        (
            bindings
                .into_iter()
                .map(|(_, binding)| binding.clone())
                .collect(),
            pending,
        )
    }

    /// Same as `bindings_for_input`, but only looks at `keymap_bindings` instead of the whole
    /// keymap, and returns each binding with its keymap index.
    ///
    /// `keymap_bindings` must be in reverse keymap order and include every binding that could
    /// match `input`. For example, leaving out a `null` binding would make the binding it disables
    /// look like it wins. `bindings_for_input` passes the whole keymap, which always meets this but
    /// goes through every binding on every call. Passing a pre-filtered list is cheaper when
    /// resolving many inputs against it.
    fn resolve_bindings_for_input<'a>(
        &self,
        input: &[impl AsKeystroke],
        context_stack: &[KeyContext],
        held: &[SharedString],
        keymap_bindings: impl IntoIterator<Item = (usize, &'a KeyBinding)>,
        include_pending: bool,
    ) -> (SmallVec<[(BindingIndex, &'a KeyBinding); 1]>, bool) {
        let mut matched_bindings = SmallVec::<[(usize, BindingIndex, &KeyBinding); 1]>::new();
        let mut pending_bindings = SmallVec::<[(usize, BindingIndex, &KeyBinding); 1]>::new();

        for (ix, binding) in keymap_bindings {
            if !binding.when_held().iter().all(|name| held.contains(name)) {
                continue;
            }
            let Some(depth) = self.binding_enabled(binding, context_stack) else {
                continue;
            };
            let Some(pending) = binding.match_keystrokes(input) else {
                continue;
            };

            if !pending {
                matched_bindings.push((depth, BindingIndex(ix), binding));
            } else if include_pending {
                pending_bindings.push((depth, BindingIndex(ix), binding));
            }
        }

        matched_bindings.sort_by(|(depth_a, ix_a, a), (depth_b, ix_b, b)| {
            depth_b
                .cmp(depth_a)
                .then(b.when_held().len().cmp(&a.when_held().len()))
                .then(ix_b.cmp(ix_a))
        });

        let has_held_candidates = matched_bindings
            .iter()
            .chain(pending_bindings.iter())
            .any(|(_, _, binding)| !binding.when_held().is_empty());
        let mut bindings: SmallVec<[_; 1]> = SmallVec::new();
        let mut first_binding_index = None;
        let mut unbound_bindings: Vec<&KeyBinding> = Vec::new();
        // A `NoAction` binding suppresses out-ranked bindings from sources with
        // equal or weaker precedence, while bindings from stronger sources (a
        // smaller meta, e.g. a user binding vs a base keymap null) still apply.
        // Bindings without a meta are treated as user bindings.
        let mut no_action_meta: Option<u32> = None;

        for (_, ix, binding) in matched_bindings {
            let meta = binding.meta.map_or(0, |meta| meta.0);
            if is_no_action(&*binding.action) {
                no_action_meta = Some(no_action_meta.map_or(meta, |existing| existing.min(meta)));
                continue;
            }

            if no_action_meta.is_some_and(|no_action_meta| meta >= no_action_meta) {
                continue;
            }

            if is_unbind(&*binding.action) {
                unbound_bindings.push(binding);
                continue;
            }

            if unbound_bindings
                .iter()
                .any(|disabled_binding| binding_is_unbound(disabled_binding, binding))
            {
                continue;
            }

            bindings.push((ix, binding));
            first_binding_index.get_or_insert(ix);
        }

        if !include_pending {
            return (bindings, false);
        }
        if has_held_candidates {
            // Resolve a candidate's completion using the same null/unbind/source and
            // specificity rules as dispatch. File order alone is insufficient here.
            let first_priority = bindings.first().map(|(ix, binding)| {
                (
                    self.binding_enabled(binding, context_stack).unwrap(),
                    binding.when_held().len(),
                    *ix,
                )
            });
            let mut full_input = SmallVec::<[&Keystroke; 4]>::new();
            for (_, _, candidate) in pending_bindings {
                if is_no_action(candidate.action()) || is_unbind(candidate.action()) {
                    continue;
                }
                full_input.clear();
                full_input.extend(input.iter().map(AsKeystroke::as_keystroke));
                full_input.extend(
                    candidate.keystrokes[input.len()..]
                        .iter()
                        .map(AsKeystroke::as_keystroke),
                );
                let (complete, _) = self.resolve_bindings_for_input(
                    &full_input,
                    context_stack,
                    held,
                    self.bindings().enumerate().rev(),
                    false,
                );
                if complete.iter().any(|(ix, binding)| {
                    let priority = (
                        self.binding_enabled(binding, context_stack).unwrap(),
                        binding.when_held().len(),
                        *ix,
                    );
                    first_priority.is_none_or(|first| {
                        if first.1 == 0 && priority.1 == 0 {
                            priority.2 >= first.2
                        } else {
                            priority >= first
                        }
                    })
                }) {
                    return (bindings, true);
                }
            }
            return (bindings, false);
        }

        // Preserve upstream prefix behavior when no held conditions are active.
        let mut pending = HashSet::default();
        for (_, ix, binding) in pending_bindings.into_iter().rev() {
            if first_binding_index.is_some_and(|binding_ix| binding_ix > ix) {
                continue;
            }
            if is_no_action(binding.action()) || is_unbind(binding.action()) {
                pending.remove(&&binding.keystrokes);
                continue;
            }
            pending.insert(&binding.keystrokes);
        }

        (bindings, !pending.is_empty())
    }
    /// Check if the given binding is enabled, given a certain key context.
    /// Returns the deepest depth at which the binding matches, or None if it doesn't match.
    fn binding_enabled(&self, binding: &KeyBinding, contexts: &[KeyContext]) -> Option<usize> {
        if let Some(predicate) = &binding.context_predicate {
            predicate.depth_of(contexts)
        } else {
            Some(contexts.len())
        }
    }

    /// Find the bindings that can follow the current input sequence, in precedence order. Only
    /// includes bindings that dispatch would run for their full keystrokes, so bindings disabled
    /// with `null` or `Unbind` are left out.
    pub fn possible_next_bindings_for_input(
        &self,
        input: &[Keystroke],
        context_stack: &[KeyContext],
    ) -> Vec<KeyBinding> {
        self.possible_next_bindings_for_input_with_held(input, context_stack, &[])
    }

    /// Find eligible completions using the same held conditions as dispatch.
    pub fn possible_next_bindings_for_input_with_held(
        &self,
        input: &[Keystroke],
        context_stack: &[KeyContext],
        held: &[SharedString],
    ) -> Vec<KeyBinding> {
        // Keeps `null` and `Unbind` bindings, which can disable candidates.
        let bindings_extending_input = self
            .bindings()
            .enumerate()
            .rev()
            .filter(|(_, binding)| {
                binding
                    .match_keystrokes(input)
                    .is_some_and(|pending| pending)
            })
            .collect::<Vec<_>>();

        let mut full_input = SmallVec::<[&Keystroke; 4]>::new();
        let mut candidates = bindings_extending_input
            .iter()
            .filter(|(_, binding)| !is_no_action(&*binding.action) && !is_unbind(&*binding.action))
            .filter_map(|&(ix, binding)| {
                if self.binding_uses_held_modifier(binding, context_stack) {
                    return None;
                }
                let depth = self.binding_enabled(binding, context_stack)?;
                // Use the typed input rather than the binding's own keystrokes. A typed keystroke
                // can match through its physical key, like `alt-s`, or the character it produced,
                // like `ß`.
                full_input.clear();
                full_input.extend(input.iter());
                full_input.extend(
                    binding.keystrokes[input.len()..]
                        .iter()
                        .map(AsKeystroke::as_keystroke),
                );
                let (dispatched, _) = self.resolve_bindings_for_input(
                    &full_input,
                    context_stack,
                    held,
                    bindings_extending_input.iter().copied(),
                    false,
                );
                dispatched
                    .iter()
                    .any(|(dispatched_ix, _)| *dispatched_ix == BindingIndex(ix))
                    .then_some((depth, ix, binding))
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable_by(|(depth_a, ix_a, a), (depth_b, ix_b, b)| {
            depth_b
                .cmp(depth_a)
                .then(b.when_held().len().cmp(&a.when_held().len()))
                .then(ix_b.cmp(ix_a))
        });

        candidates
            .into_iter()
            .map(|(_, _, binding)| binding.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate as gpui;
    use gpui::{NoAction, Unbind};

    actions!(
        test_only,
        [ActionAlpha, ActionBeta, ActionGamma, ActionDelta,]
    );

    #[test]
    fn held_modifier_only_keys_are_not_advertised_as_actions() {
        let map = Keymap::new(vec![
            KeyBinding::new("ctrl-f", ActionAlpha, None),
            KeyBinding::new("ctrl-f x", ActionBeta, None),
            KeyBinding::new("ctrl-f", NoAction, Some("Editor"))
                .with_held_modifier("word".into())
                .unwrap(),
        ]);
        let editor = [KeyContext::parse("Editor").unwrap()];
        let terminal = [KeyContext::parse("Terminal").unwrap()];
        assert_eq!(
            map.bindings_for_action_with_held(&ActionAlpha, &editor, &[])
                .count(),
            0
        );
        assert_eq!(
            map.bindings_for_action_with_held(&ActionAlpha, &terminal, &[])
                .count(),
            1
        );
        assert_eq!(
            map.bindings_for_action_with_held(&ActionBeta, &editor, &[])
                .count(),
            0
        );
        assert!(
            map.possible_next_bindings_for_input(&[Keystroke::parse("ctrl-f").unwrap()], &editor)
                .is_empty()
        );
    }

    #[test]
    fn held_unrelated_names_preserve_ordinary_prefix_precedence() {
        let map = Keymap::new(vec![
            KeyBinding::new("ctrl-x y", ActionAlpha, Some("Editor")),
            KeyBinding::new("ctrl-x", ActionBeta, Some("Workspace")),
        ]);
        let contexts = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Editor").unwrap(),
        ];
        let input = [Keystroke::parse("ctrl-x").unwrap()];
        assert!(!map.bindings_for_input(&input, &contexts).1);
        assert!(
            !map.bindings_for_input_with_held(&input, &contexts, &["unrelated".into()])
                .1
        );
    }

    #[test]
    fn held_native_menu_shortcuts_are_always_unconditional() {
        let mut map = Keymap::new(vec![
            KeyBinding::new("ctrl-alt-9", ActionAlpha, None).with_when_held(["word".into()]),
        ]);
        assert_eq!(map.bindings_for_native_menu_action(&ActionAlpha).count(), 0);
        map.add_bindings([KeyBinding::new("cmd-s", ActionAlpha, None)]);
        let native = map
            .bindings_for_native_menu_action(&ActionAlpha)
            .collect::<Vec<_>>();
        assert_eq!(native.len(), 1);
        assert_eq!(native[0].keystrokes()[0].key(), "s");
        map.add_bindings([KeyBinding::new(
            "cmd-s",
            Unbind("test_only::ActionAlpha".into()),
            None,
        )]);
        assert_eq!(map.bindings_for_native_menu_action(&ActionAlpha).count(), 0);
    }

    #[test]
    fn held_action_discovery_obeys_conditions_and_specificity() {
        let map = Keymap::new(vec![
            KeyBinding::new("ctrl-l", ActionAlpha, Some("Editor"))
                .with_when_held(["word".into(), "select".into()]),
            KeyBinding::new("ctrl-l", NoAction, Some("Editor")),
            KeyBinding::new("ctrl-j", ActionAlpha, Some("Editor")),
            KeyBinding::new("ctrl-k", ActionAlpha, Some("Editor")).with_when_held(["word".into()]),
            KeyBinding::new("ctrl-f", NoAction, Some("Editor"))
                .with_held_modifier("word".into())
                .unwrap(),
        ]);
        assert_eq!(
            map.bindings().len(),
            4,
            "declarations are not action bindings"
        );
        let context = [KeyContext::parse("Editor").unwrap()];
        let ordinary = map
            .bindings_for_action_with_held(&ActionAlpha, &context, &[])
            .collect::<Vec<_>>();
        assert_eq!(ordinary.len(), 1);
        assert_eq!(ordinary[0].keystrokes()[0].key(), "j");
        let held = ["word".into(), "select".into()];
        let active = map
            .bindings_for_action_with_held(&ActionAlpha, &context, &held)
            .collect::<Vec<_>>();
        assert_eq!(
            active.len(),
            3,
            "a later ordinary null does not hide the more specific action"
        );
    }

    #[test]
    fn held_pending_resolution_preserves_typed_alias_and_source_precedence() {
        let map = Keymap::new(vec![
            KeyBinding::new("alt-s x", ActionAlpha, Some("Editor"))
                .with_when_held(["word".into()])
                .with_meta(crate::KeyBindingMetaIndex(0)),
            KeyBinding::new("ß x", NoAction, Some("Editor"))
                .with_when_held(["word".into()])
                .with_meta(crate::KeyBindingMetaIndex(1)),
        ]);
        let context = [KeyContext::parse("Editor").unwrap()];
        let input = [Keystroke::parse("alt-s->ß").unwrap()];
        let held = ["word".into()];
        assert!(map.bindings_for_input_with_held(&input, &context, &held).1);
        let next = map.possible_next_bindings_for_input_with_held(&input, &context, &held);
        assert_eq!(next.len(), 1);
        assert!(next[0].action().partial_eq(&ActionAlpha));
    }

    #[test]
    fn held_regression_irrelevant_short_unbind_does_not_block_chord() {
        let map = Keymap::new(vec![
            KeyBinding::new("ctrl-x ctrl-y", ActionBeta, Some("Editor"))
                .with_when_held(["word".into()]),
            KeyBinding::new(
                "ctrl-x",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor"),
            )
            .with_when_held(["word".into(), "select".into()]),
        ]);
        let contexts = [KeyContext::parse("Editor").unwrap()];
        let held = ["word".into(), "select".into()];
        let input = [
            Keystroke::parse("ctrl-x").unwrap(),
            Keystroke::parse("ctrl-y").unwrap(),
        ];
        let (full, _) = map.bindings_for_input_with_held(&input, &contexts, &held);
        assert!(full[0].action().partial_eq(&ActionBeta));
        assert!(
            map.bindings_for_input_with_held(&input[..1], &contexts, &held)
                .1,
            "unbinding an absent short action must not prevent an unrelated chord"
        );
    }

    #[test]
    fn held_regression_held_sequence_specificity() {
        let map = Keymap::new(vec![
            KeyBinding::new("ctrl-x ctrl-y", ActionAlpha, Some("Editor"))
                .with_when_held(["word".into(), "select".into()]),
            KeyBinding::new("ctrl-x ctrl-y", NoAction, Some("Editor"))
                .with_when_held(["word".into()]),
        ]);
        let contexts = [KeyContext::parse("Editor").unwrap()];
        let held = ["word".into(), "select".into()];
        let input = [
            Keystroke::parse("ctrl-x").unwrap(),
            Keystroke::parse("ctrl-y").unwrap(),
        ];
        let (full, _) = map.bindings_for_input_with_held(&input, &contexts, &held);
        assert!(full[0].action().partial_eq(&ActionAlpha));
        assert!(
            map.bindings_for_input_with_held(&input[..1], &contexts, &held)
                .1,
            "the winning held sequence must be allowed to start"
        );
    }

    #[test]
    fn held_bindings_specificity_fallback_context_and_sequences() {
        let context = [KeyContext::parse("Editor mode=full").unwrap()];
        let mut map = Keymap::new(vec![
            KeyBinding::new("ctrl-l", ActionDelta, Some("Editor"))
                .with_when_held(["word".into(), "select".into()]),
            KeyBinding::new("ctrl-l", ActionBeta, Some("Editor")).with_when_held(["word".into()]),
            KeyBinding::new("ctrl-l", ActionGamma, Some("Editor"))
                .with_when_held(["select".into()]),
            // Deliberately put the ordinary bindings and sequence last.
            KeyBinding::new("ctrl-l", ActionAlpha, Some("Editor")),
            KeyBinding::new("ctrl-l x", ActionAlpha, Some("Editor")),
        ]);
        let input = [Keystroke::parse("ctrl-l").unwrap()];
        for (names, action) in [
            (vec!["word".into()], &ActionBeta as &dyn Action),
            (vec!["select".into()], &ActionGamma as &dyn Action),
            (
                vec!["word".into(), "select".into()],
                &ActionDelta as &dyn Action,
            ),
            (
                vec!["select".into(), "word".into(), "extra".into()],
                &ActionDelta as &dyn Action,
            ),
        ] {
            let (bindings, pending) = map.bindings_for_input_with_held(&input, &context, &names);
            assert!(bindings[0].action().partial_eq(action));
            assert!(
                !pending,
                "ordinary sequences must not delay a more specific held binding"
            );
        }
        let (bindings, pending) = map.bindings_for_input(&input, &context);
        assert!(bindings[0].action().partial_eq(&ActionAlpha));
        assert!(pending); // Existing multi-stroke semantics are unchanged.
        assert!(
            map.bindings_for_input_with_held(
                &input,
                &[KeyContext::parse("Terminal").unwrap()],
                &["word".into()]
            )
            .0
            .is_empty()
        );
        map.add_bindings([
            KeyBinding::new("ctrl-l", ActionAlpha, Some("Editor")).with_when_held(["word".into()])
        ]);
        assert!(
            map.bindings_for_input_with_held(&input, &context, &["word".into()])
                .0[0]
                .action()
                .partial_eq(&ActionAlpha)
        );
    }

    #[test]
    fn held_bindings_keep_depth_precedence_and_conditional_nulls() {
        let contexts = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Editor").unwrap(),
        ];
        let mut map = Keymap::new(vec![
            KeyBinding::new("ctrl-l", ActionAlpha, Some("Editor")),
            KeyBinding::new("ctrl-l", ActionBeta, Some("Workspace"))
                .with_when_held(["word".into(), "select".into()]),
        ]);
        let input = [Keystroke::parse("ctrl-l").unwrap()];
        assert!(
            map.bindings_for_input_with_held(&input, &contexts, &["word".into(), "select".into()])
                .0[0]
                .action()
                .partial_eq(&ActionAlpha)
        );
        map.add_bindings([
            KeyBinding::new("ctrl-l", NoAction, Some("Editor")).with_when_held(["word".into()])
        ]);
        assert!(
            map.bindings_for_input_with_held(&input, &contexts, &["word".into()])
                .0
                .is_empty()
        );
        assert!(
            map.bindings_for_input(&input, &contexts).0[0]
                .action()
                .partial_eq(&ActionAlpha)
        );
        assert_eq!(map.bindings_for_action(&ActionAlpha).count(), 1);
    }

    #[test]
    fn test_keymap() {
        let bindings = [
            KeyBinding::new("ctrl-a", ActionAlpha {}, None),
            KeyBinding::new("ctrl-a", ActionBeta {}, Some("pane")),
            KeyBinding::new("ctrl-a", ActionGamma {}, Some("editor && mode==full")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings.clone());

        // global bindings are enabled in all contexts
        assert_eq!(keymap.binding_enabled(&bindings[0], &[]), Some(0));
        assert_eq!(
            keymap.binding_enabled(&bindings[0], &[KeyContext::parse("terminal").unwrap()]),
            Some(1)
        );

        // contextual bindings are enabled in contexts that match their predicate
        assert_eq!(
            keymap.binding_enabled(&bindings[1], &[KeyContext::parse("barf x=y").unwrap()]),
            None
        );
        assert_eq!(
            keymap.binding_enabled(&bindings[1], &[KeyContext::parse("pane x=y").unwrap()]),
            Some(1)
        );

        assert_eq!(
            keymap.binding_enabled(&bindings[2], &[KeyContext::parse("editor").unwrap()]),
            None
        );
        assert_eq!(
            keymap.binding_enabled(
                &bindings[2],
                &[KeyContext::parse("editor mode=full").unwrap()]
            ),
            Some(1)
        );
    }

    #[test]
    fn test_depth_precedence() {
        let bindings = [
            KeyBinding::new("ctrl-a", ActionBeta {}, Some("pane")),
            KeyBinding::new("ctrl-a", ActionGamma {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-a").unwrap()],
            &[
                KeyContext::parse("pane").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ],
        );

        assert!(!pending);
        assert_eq!(result.len(), 2);
        assert!(result[0].action.partial_eq(&ActionGamma {}));
        assert!(result[1].action.partial_eq(&ActionBeta {}));
    }

    #[test]
    fn test_keymap_disabled() {
        let bindings = [
            KeyBinding::new("ctrl-a", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-b", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-a", NoAction {}, Some("editor && mode==full")),
            KeyBinding::new("ctrl-b", NoAction {}, None),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // binding is only enabled in a specific context
        assert!(
            keymap
                .bindings_for_input(
                    &[Keystroke::parse("ctrl-a").unwrap()],
                    &[KeyContext::parse("barf").unwrap()],
                )
                .0
                .is_empty()
        );
        assert!(
            !keymap
                .bindings_for_input(
                    &[Keystroke::parse("ctrl-a").unwrap()],
                    &[KeyContext::parse("editor").unwrap()],
                )
                .0
                .is_empty()
        );

        // binding is disabled in a more specific context
        assert!(
            keymap
                .bindings_for_input(
                    &[Keystroke::parse("ctrl-a").unwrap()],
                    &[KeyContext::parse("editor mode=full").unwrap()],
                )
                .0
                .is_empty()
        );

        // binding is globally disabled
        assert!(
            keymap
                .bindings_for_input(
                    &[Keystroke::parse("ctrl-b").unwrap()],
                    &[KeyContext::parse("barf").unwrap()],
                )
                .0
                .is_empty()
        );
    }

    #[test]
    /// Tests for https://github.com/zed-industries/zed/issues/30259
    fn test_multiple_keystroke_binding_disabled() {
        let bindings = [
            KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w w", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let space = || Keystroke::parse("space").unwrap();
        let w = || Keystroke::parse("w").unwrap();

        let space_w = [space(), w()];
        let space_w_w = [space(), w(), w()];

        let workspace_context = || [KeyContext::parse("workspace").unwrap()];

        let editor_workspace_context = || {
            [
                KeyContext::parse("workspace").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ]
        };

        // Ensure `space` results in pending input on the workspace, but not editor
        let space_workspace = keymap.bindings_for_input(&[space()], &workspace_context());
        assert!(space_workspace.0.is_empty());
        assert!(space_workspace.1);

        let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
        assert!(space_editor.0.is_empty());
        assert!(!space_editor.1);

        // Ensure `space w` results in pending input on the workspace, but not editor
        let space_w_workspace = keymap.bindings_for_input(&space_w, &workspace_context());
        assert!(space_w_workspace.0.is_empty());
        assert!(space_w_workspace.1);

        let space_w_editor = keymap.bindings_for_input(&space_w, &editor_workspace_context());
        assert!(space_w_editor.0.is_empty());
        assert!(!space_w_editor.1);

        // Ensure `space w w` results in the binding in the workspace, but not in the editor
        let space_w_w_workspace = keymap.bindings_for_input(&space_w_w, &workspace_context());
        assert!(!space_w_w_workspace.0.is_empty());
        assert!(!space_w_w_workspace.1);

        let space_w_w_editor = keymap.bindings_for_input(&space_w_w, &editor_workspace_context());
        assert!(space_w_w_editor.0.is_empty());
        assert!(!space_w_w_editor.1);

        // Now test what happens if we have another binding defined AFTER the NoAction
        // that should result in pending
        let bindings = [
            KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w w", NoAction {}, Some("editor")),
            KeyBinding::new("space w x", ActionAlpha {}, Some("editor")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
        assert!(space_editor.0.is_empty());
        assert!(space_editor.1);

        // Now test what happens if we have another binding defined BEFORE the NoAction
        // that should result in pending
        let bindings = [
            KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w x", ActionAlpha {}, Some("editor")),
            KeyBinding::new("space w w", NoAction {}, Some("editor")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
        assert!(space_editor.0.is_empty());
        assert!(space_editor.1);

        // Now test what happens if we have another binding defined at a higher context
        // that should result in pending
        let bindings = [
            KeyBinding::new("space w w", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w x", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("space w w", NoAction {}, Some("editor")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let space_editor = keymap.bindings_for_input(&[space()], &editor_workspace_context());
        assert!(space_editor.0.is_empty());
        assert!(space_editor.1);
    }

    #[test]
    fn test_override_multikey() {
        let bindings = [
            KeyBinding::new("ctrl-w left", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-w", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-w").unwrap()],
            &[KeyContext::parse("editor").unwrap()],
        );
        assert!(result.is_empty());
        assert!(pending);

        let bindings = [
            KeyBinding::new("ctrl-w left", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-w", ActionBeta {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-w").unwrap()],
            &[KeyContext::parse("editor").unwrap()],
        );
        assert_eq!(result.len(), 1);
        assert!(!pending);
    }

    #[test]
    fn test_simple_disable() {
        let bindings = [
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x").unwrap()],
            &[KeyContext::parse("editor").unwrap()],
        );
        assert!(result.is_empty());
        assert!(!pending);
    }

    #[test]
    fn test_disable_weaker_sources_only() {
        const USER: KeyBindingMetaIndex = KeyBindingMetaIndex(0);
        const VIM: KeyBindingMetaIndex = KeyBindingMetaIndex(1);
        const BASE: KeyBindingMetaIndex = KeyBindingMetaIndex(2);
        const DEFAULT: KeyBindingMetaIndex = KeyBindingMetaIndex(3);

        let editor_context = || [KeyContext::parse("editor").unwrap()];
        let ctrl_x = || [Keystroke::parse("ctrl-x").unwrap()];

        // A base keymap null disables a default binding in the same context.
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(BASE),
        ]);
        let (result, _) = keymap.bindings_for_input(&ctrl_x(), &editor_context());
        assert!(result.is_empty());

        // A user binding is not affected by base keymap or default nulls.
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(BASE),
            KeyBinding::new("ctrl-x", ActionBeta {}, None).with_meta(USER),
        ]);
        let (result, _) = keymap.bindings_for_input(&ctrl_x(), &editor_context());
        assert_eq!(result.len(), 1);
        assert!(result[0].action.partial_eq(&ActionBeta {}));

        // A user binding at a shallower context is not disabled by a deeper
        // base keymap null.
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(BASE),
            KeyBinding::new("ctrl-x", ActionBeta {}, Some("workspace")).with_meta(USER),
        ]);
        let (result, _) = keymap.bindings_for_input(
            &ctrl_x(),
            &[
                KeyContext::parse("workspace").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ],
        );
        assert_eq!(result.len(), 1);
        assert!(result[0].action.partial_eq(&ActionBeta {}));

        // A vim binding survives a base keymap null, and a user null disables
        // everything.
        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(BASE),
            KeyBinding::new("ctrl-x", ActionGamma {}, Some("editor")).with_meta(VIM),
        ]);
        let (result, _) = keymap.bindings_for_input(&ctrl_x(), &editor_context());
        assert_eq!(result.len(), 1);
        assert!(result[0].action.partial_eq(&ActionGamma {}));

        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-x", ActionGamma {}, Some("editor")).with_meta(VIM),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")).with_meta(USER),
        ]);
        let (result, _) = keymap.bindings_for_input(&ctrl_x(), &editor_context());
        assert!(result.is_empty());
    }

    #[test]
    fn test_fail_to_disable() {
        // disabled at the wrong level
        let bindings = [
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("editor")),
            KeyBinding::new("ctrl-x", NoAction {}, Some("workspace")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x").unwrap()],
            &[
                KeyContext::parse("workspace").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ],
        );
        assert_eq!(result.len(), 1);
        assert!(!pending);
    }

    #[test]
    fn test_disable_deeper() {
        let bindings = [
            KeyBinding::new("ctrl-x", ActionAlpha {}, Some("workspace")),
            KeyBinding::new("ctrl-x", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Ensure `space` results in pending input on the workspace, but not editor
        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x").unwrap()],
            &[
                KeyContext::parse("workspace").unwrap(),
                KeyContext::parse("editor").unwrap(),
            ],
        );
        assert_eq!(result.len(), 0);
        assert!(!pending);
    }

    #[test]
    fn test_pending_match_enabled() {
        let bindings = [
            KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
            KeyBinding::new("ctrl-x 0", ActionAlpha, Some("Workspace")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let matched = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
            &[
                KeyContext::parse("Workspace"),
                KeyContext::parse("Pane"),
                KeyContext::parse("Editor vim_mode=normal"),
            ]
            .map(Result::unwrap),
        );
        assert_eq!(matched.0.len(), 1);
        assert!(matched.0[0].action.partial_eq(&ActionBeta));
        assert!(matched.1);
    }

    #[test]
    fn test_pending_match_enabled_extended() {
        let bindings = [
            KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
            KeyBinding::new("ctrl-x 0", NoAction, Some("Workspace")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let matched = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
            &[
                KeyContext::parse("Workspace"),
                KeyContext::parse("Pane"),
                KeyContext::parse("Editor vim_mode=normal"),
            ]
            .map(Result::unwrap),
        );
        assert_eq!(matched.0.len(), 1);
        assert!(matched.0[0].action.partial_eq(&ActionBeta));
        assert!(!matched.1);
        let bindings = [
            KeyBinding::new("ctrl-x", ActionBeta, Some("Workspace")),
            KeyBinding::new("ctrl-x 0", NoAction, Some("vim_mode == normal")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let matched = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
            &[
                KeyContext::parse("Workspace"),
                KeyContext::parse("Pane"),
                KeyContext::parse("Editor vim_mode=normal"),
            ]
            .map(Result::unwrap),
        );
        assert_eq!(matched.0.len(), 1);
        assert!(matched.0[0].action.partial_eq(&ActionBeta));
        assert!(!matched.1);
    }

    #[test]
    fn test_overriding_prefix() {
        let bindings = [
            KeyBinding::new("ctrl-x 0", ActionAlpha, Some("Workspace")),
            KeyBinding::new("ctrl-x", ActionBeta, Some("vim_mode == normal")),
        ];
        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let matched = keymap.bindings_for_input(
            &[Keystroke::parse("ctrl-x")].map(Result::unwrap),
            &[
                KeyContext::parse("Workspace"),
                KeyContext::parse("Pane"),
                KeyContext::parse("Editor vim_mode=normal"),
            ]
            .map(Result::unwrap),
        );
        assert_eq!(matched.0.len(), 1);
        assert!(matched.0[0].action.partial_eq(&ActionBeta));
        assert!(!matched.1);
    }

    #[test]
    fn test_context_precedence_with_same_source() {
        // Test case: User has both Workspace and Editor bindings for the same key
        // Editor binding should take precedence over Workspace binding
        let bindings = [
            KeyBinding::new("cmd-r", ActionAlpha {}, Some("Workspace")),
            KeyBinding::new("cmd-r", ActionBeta {}, Some("Editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        // Test with context stack: [Workspace, Editor] (Editor is deeper)
        let (result, _) = keymap.bindings_for_input(
            &[Keystroke::parse("cmd-r").unwrap()],
            &[
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Editor").unwrap(),
            ],
        );

        // Both bindings should be returned, but Editor binding should be first (highest precedence)
        assert_eq!(result.len(), 2);
        assert!(result[0].action.partial_eq(&ActionBeta {})); // Editor binding first
        assert!(result[1].action.partial_eq(&ActionAlpha {})); // Workspace binding second
    }

    #[test]
    fn test_bindings_for_action() {
        let bindings = [
            KeyBinding::new("ctrl-a", ActionAlpha {}, Some("pane")),
            KeyBinding::new("ctrl-b", ActionBeta {}, Some("editor && mode == full")),
            KeyBinding::new("ctrl-c", ActionGamma {}, Some("workspace")),
            KeyBinding::new("ctrl-a", NoAction {}, Some("pane && active")),
            KeyBinding::new("ctrl-b", NoAction {}, Some("editor")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        assert_bindings(&keymap, &ActionAlpha {}, &["ctrl-a"]);
        assert_bindings(&keymap, &ActionBeta {}, &[]);
        assert_bindings(&keymap, &ActionGamma {}, &["ctrl-c"]);

        #[track_caller]
        fn assert_bindings(keymap: &Keymap, action: &dyn Action, expected: &[&str]) {
            let actual = keymap
                .bindings_for_action(action)
                .map(|binding| {
                    binding.keystrokes[0]
                        .inner()
                        .unparse(crate::OperatingSystem::Mac)
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{:?}", action);
        }
    }

    #[test]
    fn test_targeted_unbind_ignores_target_context() {
        let bindings = [
            KeyBinding::new("tab", ActionAlpha {}, Some("Editor")),
            KeyBinding::new("tab", ActionBeta {}, Some("Editor && showing_completions")),
            KeyBinding::new(
                "tab",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor && edit_prediction"),
            ),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        let (result, pending) = keymap.bindings_for_input(
            &[Keystroke::parse("tab").unwrap()],
            &[KeyContext::parse("Editor showing_completions edit_prediction").unwrap()],
        );

        assert!(!pending);
        assert_eq!(result.len(), 1);
        assert!(result[0].action.partial_eq(&ActionBeta {}));
    }

    #[test]
    fn test_bindings_for_action_keeps_binding_for_narrower_targeted_unbind() {
        let bindings = [
            KeyBinding::new("tab", ActionAlpha {}, Some("Editor")),
            KeyBinding::new(
                "tab",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor && edit_prediction"),
            ),
            KeyBinding::new("tab", ActionBeta {}, Some("Editor && showing_completions")),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        assert_bindings(&keymap, &ActionAlpha {}, &["tab"]);
        assert_bindings(&keymap, &ActionBeta {}, &["tab"]);

        #[track_caller]
        fn assert_bindings(keymap: &Keymap, action: &dyn Action, expected: &[&str]) {
            let actual = keymap
                .bindings_for_action(action)
                .map(|binding| {
                    binding.keystrokes[0]
                        .inner()
                        .unparse(crate::OperatingSystem::Mac)
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{:?}", action);
        }
    }

    #[test]
    fn test_bindings_for_action_removes_binding_for_broader_targeted_unbind() {
        let bindings = [
            KeyBinding::new("tab", ActionAlpha {}, Some("Editor && edit_prediction")),
            KeyBinding::new(
                "tab",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor"),
            ),
        ];

        let mut keymap = Keymap::default();
        keymap.add_bindings(bindings);

        assert!(keymap.bindings_for_action(&ActionAlpha {}).next().is_none());
    }

    #[test]
    fn test_source_precedence_sorting() {
        // KeybindSource precedence: User (0) > Vim (1) > Base (2) > Default (3)
        // Test that user keymaps take precedence over default keymaps at the same context depth
        let mut keymap = Keymap::default();

        // Add a default keymap binding first
        let mut default_binding = KeyBinding::new("cmd-r", ActionAlpha {}, Some("Editor"));
        default_binding.set_meta(KeyBindingMetaIndex(3)); // Default source
        keymap.add_bindings([default_binding]);

        // Add a user keymap binding
        let mut user_binding = KeyBinding::new("cmd-r", ActionBeta {}, Some("Editor"));
        user_binding.set_meta(KeyBindingMetaIndex(0)); // User source
        keymap.add_bindings([user_binding]);

        // Test with Editor context stack
        let (result, _) = keymap.bindings_for_input(
            &[Keystroke::parse("cmd-r").unwrap()],
            &[KeyContext::parse("Editor").unwrap()],
        );

        // User binding should take precedence over default binding
        assert_eq!(result.len(), 2);
        assert!(result[0].action.partial_eq(&ActionBeta {}));
        assert!(result[1].action.partial_eq(&ActionAlpha {}));
    }

    #[test]
    fn test_possible_next_bindings_exclude_null_bindings() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("ctrl-k a", ActionAlpha {}, Some("Editor")),
            KeyBinding::new("ctrl-k b", ActionBeta {}, Some("Editor")),
            KeyBinding::new("ctrl-k a", NoAction {}, Some("Editor")),
        ]);

        let bindings = keymap.possible_next_bindings_for_input(
            &[Keystroke::parse("ctrl-k").unwrap()],
            &[KeyContext::parse("Editor").unwrap()],
        );
        assert_eq!(
            describe_bindings(&bindings),
            vec![("ctrl-k b".to_string(), "test_only::ActionBeta")]
        );
    }

    #[test]
    fn test_possible_next_bindings_exclude_unbound_bindings() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("ctrl-k a", ActionAlpha {}, Some("Editor")),
            KeyBinding::new("ctrl-k b", ActionBeta {}, Some("Editor")),
            KeyBinding::new(
                "ctrl-k a",
                Unbind("test_only::ActionAlpha".into()),
                Some("Editor"),
            ),
        ]);

        let bindings = keymap.possible_next_bindings_for_input(
            &[Keystroke::parse("ctrl-k").unwrap()],
            &[KeyContext::parse("Editor").unwrap()],
        );
        assert_eq!(
            describe_bindings(&bindings),
            vec![("ctrl-k b".to_string(), "test_only::ActionBeta")]
        );
    }

    #[test]
    fn test_possible_next_bindings_keep_user_binding_over_base_keymap_null() {
        const USER: KeyBindingMetaIndex = KeyBindingMetaIndex(0);
        const BASE: KeyBindingMetaIndex = KeyBindingMetaIndex(2);
        const DEFAULT: KeyBindingMetaIndex = KeyBindingMetaIndex(3);

        let mut keymap = Keymap::default();
        keymap.add_bindings([
            KeyBinding::new("ctrl-k a", ActionAlpha {}, Some("Editor")).with_meta(DEFAULT),
            KeyBinding::new("ctrl-k a", NoAction {}, Some("Editor")).with_meta(BASE),
            KeyBinding::new("ctrl-k a", ActionBeta {}, Some("Editor")).with_meta(USER),
        ]);

        let bindings = keymap.possible_next_bindings_for_input(
            &[Keystroke::parse("ctrl-k").unwrap()],
            &[KeyContext::parse("Editor").unwrap()],
        );
        assert_eq!(
            describe_bindings(&bindings),
            vec![("ctrl-k a".to_string(), "test_only::ActionBeta")]
        );
    }

    #[test]
    fn test_possible_next_bindings_apply_disabling_by_context() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("ctrl-k a", ActionAlpha {}, Some("Workspace")),
            KeyBinding::new("ctrl-k a", ActionBeta {}, Some("Editor")),
            KeyBinding::new("ctrl-k b", ActionGamma {}, Some("Editor")),
            // Only removes the editor binding, so the workspace binding still dispatches.
            KeyBinding::new(
                "ctrl-k a",
                Unbind("test_only::ActionBeta".into()),
                Some("Editor"),
            ),
            // Doesn't apply because `Terminal` isn't in the context stack.
            KeyBinding::new("ctrl-k b", NoAction {}, Some("Terminal")),
        ]);

        let bindings = keymap.possible_next_bindings_for_input(
            &[Keystroke::parse("ctrl-k").unwrap()],
            &[
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Editor").unwrap(),
            ],
        );
        assert_eq!(
            describe_bindings(&bindings),
            vec![
                ("ctrl-k b".to_string(), "test_only::ActionGamma"),
                ("ctrl-k a".to_string(), "test_only::ActionAlpha"),
            ]
        );
    }

    #[test]
    fn test_possible_next_bindings_exclude_longer_chord_disabled_in_deeper_context() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("space w w", ActionAlpha {}, Some("Workspace")),
            KeyBinding::new("space w x", ActionBeta {}, Some("Workspace")),
            KeyBinding::new("space w w", NoAction {}, Some("AgentPanel")),
        ]);
        let context_stack = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("AgentPanel").unwrap(),
        ];

        for input in ["space", "space w"] {
            let input = input
                .split(' ')
                .map(|keystroke| Keystroke::parse(keystroke).unwrap())
                .collect::<Vec<_>>();
            let bindings = keymap.possible_next_bindings_for_input(&input, &context_stack);
            assert_eq!(
                describe_bindings(&bindings),
                vec![("space w x".to_string(), "test_only::ActionBeta")]
            );
        }
    }

    #[test]
    fn test_possible_next_bindings_exclude_disabled_bindings_with_typed_input() {
        // Option-S reports both its physical key and the character it produced, so typed input
        // matches bindings written with either.
        let typed_input = [Keystroke::parse("alt-s->ß").unwrap()];
        let keymap = Keymap::new(vec![
            KeyBinding::new("ß a", ActionAlpha {}, None),
            KeyBinding::new("alt-s a", NoAction {}, None),
        ]);
        assert!(
            keymap
                .possible_next_bindings_for_input(&typed_input, &[])
                .is_empty()
        );

        // Cross-check against dispatch of the same typed input.
        let typed_then_a = [
            Keystroke::parse("alt-s->ß").unwrap(),
            Keystroke::parse("a").unwrap(),
        ];
        let (dispatched, _) = keymap.bindings_for_input(&typed_then_a, &[]);
        assert!(dispatched.is_empty());
    }

    fn describe_bindings(bindings: &[KeyBinding]) -> Vec<(String, &'static str)> {
        bindings
            .iter()
            .map(|binding| {
                (
                    binding
                        .keystrokes()
                        .iter()
                        .map(|keystroke| keystroke.inner().unparse(crate::OperatingSystem::Mac))
                        .collect::<Vec<_>>()
                        .join(" "),
                    binding.action().name(),
                )
            })
            .collect()
    }
}

use crate::{
    AsKeystroke, KeyBinding, KeyContext, Keymap, KeymapVersion, Keystroke, Modifiers, SharedString,
};
use smallvec::SmallVec;

/// Per-window state. Cancelled presses remain consumed until key-up (or a fresh,
/// non-repeat key-down), so releasing Control first cannot turn a repeat into text.
#[derive(Default)]
pub(crate) struct HeldModifiers {
    pressed: SmallVec<[HeldPress; 2]>,
    active: SmallVec<[SharedString; 2]>,
    version: Option<KeymapVersion>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoAction;

    fn key(source: &'static str) -> Keystroke {
        Keystroke::parse(source).unwrap()
    }
    fn context() -> [KeyContext; 1] {
        [KeyContext::parse("Editor mode=full").unwrap()]
    }
    fn keymap() -> Keymap {
        Keymap::new(vec![
            KeyBinding::new("ctrl-f", NoAction, Some("Editor && mode == full"))
                .with_held_modifier("word".into())
                .unwrap(),
            KeyBinding::new("ctrl-d", NoAction, Some("Editor && mode == full"))
                .with_held_modifier("select".into())
                .unwrap(),
        ])
    }
    fn down(state: &mut HeldModifiers, map: &Keymap, source: &'static str) -> bool {
        state.reconcile(key(source).modifiers, &context());
        state.key_down(&key(source), false, map, &context(), true)
    }

    #[test]
    fn held_modifiers_compose_in_either_order_and_release_independently() {
        for (first, second) in [("ctrl-f", "ctrl-d"), ("ctrl-d", "ctrl-f")] {
            let map = keymap();
            let mut state = HeldModifiers::default();
            state.sync_keymap(&map);
            assert!(down(&mut state, &map, first));
            assert!(down(&mut state, &map, second));
            assert!(state.names().contains(&"word".into()));
            assert!(state.names().contains(&"select".into()));
            for _ in 0..5 {
                assert!(!down(&mut state, &map, "ctrl-l"));
                assert!(!state.key_down(&key("ctrl-l"), true, &map, &context(), true));
                assert_eq!(state.names().len(), 2);
            }
            assert!(state.key_up(&key(first)));
            assert_eq!(state.names().len(), 1);
            assert!(state.key_up(&key(second)));
            assert!(state.names().is_empty());
        }
    }

    #[test]
    fn held_modifiers_suppress_repeats_after_control_release_without_reactivation() {
        let map = keymap();
        let mut state = HeldModifiers::default();
        assert!(down(&mut state, &map, "ctrl-f"));
        assert!(state.key_down(&key("ctrl-f"), true, &map, &context(), true));
        assert!(state.reconcile(Modifiers::none(), &context()));
        assert!(state.names().is_empty());
        assert!(state.key_down(&key("f"), true, &map, &context(), true));
        state.reconcile(Modifiers::control(), &context());
        assert!(state.names().is_empty());
        assert!(state.key_up(&key("f")));
        assert!(!down(&mut state, &map, "f"));
        assert!(!down(&mut state, &map, "d"));
    }

    #[test]
    fn held_modifiers_focus_context_reload_and_missed_key_up() {
        let mut map = keymap();
        let mut state = HeldModifiers::default();
        state.sync_keymap(&map);
        assert!(down(&mut state, &map, "ctrl-f"));
        state.cancel();
        assert!(state.names().is_empty());
        assert!(state.key_down(&key("f"), true, &map, &context(), true));
        // A fresh press recovers even if the previous key-up was lost.
        assert!(!down(&mut state, &map, "f"));
        assert!(down(&mut state, &map, "ctrl-f"));
        let nested_input = [
            context()[0].clone(),
            KeyContext::parse("Editor mode=single_line").unwrap(),
        ];
        assert!(state.reconcile(Modifiers::control(), &nested_input));
        assert!(state.names().is_empty());
        assert!(!state.key_down(&key("ctrl-d"), false, &map, &nested_input, true));
        assert!(down(&mut state, &map, "ctrl-d"));
        map.clear();
        map.add_bindings([
            KeyBinding::new("ctrl-s", NoAction, Some("Editor && mode == full"))
                .with_held_modifier("word".into())
                .unwrap(),
        ]);
        assert!(state.sync_keymap(&map));
        assert!(state.names().is_empty());
        assert!(state.key_down(&key("d"), true, &map, &context(), true));
        assert!(state.key_up(&key("d")));
        assert!(!down(&mut state, &map, "ctrl-f"));
        assert!(down(&mut state, &map, "ctrl-s"));
    }

    #[test]
    fn held_modifiers_preserve_character_input_preference_and_layout_matching() {
        let mut state = HeldModifiers::default();
        let map = Keymap::new(vec![
            KeyBinding::new("alt-s", NoAction, None)
                .with_held_modifier("extra".into())
                .unwrap(),
        ]);
        assert!(!state.key_down(&key("alt-s->ß"), false, &map, &[], false));
        assert!(state.key_down(&key("alt-s->ß"), false, &map, &[], true));
        assert!(state.key_up(&key("s")));
        assert!(state.names().is_empty());
    }

    #[test]
    fn held_modifiers_support_more_than_two_and_shared_names() {
        let map = Keymap::new(
            [
                ("ctrl-a", "first"),
                ("ctrl-b", "second"),
                ("ctrl-c", "third"),
                ("ctrl-e", "fourth"),
                ("ctrl-s", "first"),
            ]
            .into_iter()
            .map(|(key, name)| {
                KeyBinding::new(key, NoAction, None)
                    .with_held_modifier(name.into())
                    .unwrap()
            })
            .collect(),
        );
        let mut state = HeldModifiers::default();
        for source in ["ctrl-a", "ctrl-b", "ctrl-c", "ctrl-e", "ctrl-s"] {
            assert!(down(&mut state, &map, source));
        }
        assert_eq!(state.names().len(), 4);
        assert!(state.key_up(&key("ctrl-a")));
        assert_eq!(state.names().len(), 4);
        assert!(state.key_up(&key("ctrl-s")));
        assert_eq!(state.names().len(), 3);
    }
}

struct HeldPress {
    key: String,
    binding: KeyBinding,
    modifiers: Modifiers,
    active: bool,
}

impl HeldModifiers {
    pub fn names(&self) -> &[SharedString] {
        &self.active
    }

    pub fn is_empty(&self) -> bool {
        self.pressed.is_empty()
    }

    pub fn consumes_key(&self, key: &Keystroke) -> bool {
        self.pressed.iter().any(|press| press.key == key.key)
    }

    /// Returns whether the keymap changed. A reload never reactivates an old press.
    pub fn sync_keymap(&mut self, keymap: &Keymap) -> bool {
        if self.version == Some(keymap.version()) {
            return false;
        }
        let changed = self.version.is_some();
        self.version = Some(keymap.version());
        self.cancel();
        changed
    }

    pub fn cancel(&mut self) {
        for press in &mut self.pressed {
            press.active = false;
        }
        self.active.clear();
    }

    /// A released native modifier or changed context cancels the press for its
    /// remaining lifetime. Pressing Control again does not resurrect it.
    pub fn reconcile(&mut self, modifiers: Modifiers, contexts: &[KeyContext]) -> bool {
        let mut changed = false;
        for press in &mut self.pressed {
            if press.active
                && (!press.modifiers.is_subset_of(&modifiers)
                    || press.binding.predicate().is_some_and(|p| !p.eval(contexts)))
            {
                press.active = false;
                changed = true;
            }
        }
        if changed {
            self.refresh_names();
        }
        changed
    }

    pub fn key_down(
        &mut self,
        key: &Keystroke,
        is_repeat: bool,
        keymap: &Keymap,
        contexts: &[KeyContext],
        allow_activation: bool,
    ) -> bool {
        if is_repeat {
            // Never activate from auto-repeat, including after a focus change.
            return self.consumes_key(key);
        }
        // A fresh down also recovers from a missed up; don't swallow future typing.
        self.key_up(key);
        if !allow_activation {
            return false;
        }
        let Some(binding) = keymap.held_modifier_for_input(key, contexts) else {
            return false;
        };
        self.pressed.push(HeldPress {
            key: key.key.clone(),
            modifiers: binding.keystrokes()[0].as_keystroke().modifiers,
            binding: binding.clone(),
            active: true,
        });
        self.refresh_names();
        true
    }

    pub fn key_up(&mut self, key: &Keystroke) -> bool {
        let len = self.pressed.len();
        self.pressed.retain(|press| press.key != key.key);
        let consumed = self.pressed.len() != len;
        if consumed {
            self.refresh_names();
        }
        consumed
    }

    fn refresh_names(&mut self) {
        self.active.clear();
        for press in &self.pressed {
            if press.active {
                let name = press.binding.held_modifier().unwrap();
                if !self.active.contains(name) {
                    self.active.push(name.clone());
                }
            }
        }
    }
}

# Held-key modifiers in this fork

This fork lets you use ordinary keys as temporary modifiers inside Zed. Configure
them in `~/.config/zed/keymap.json`. Saving the file uses Zed's existing live reload;
you do not need to rebuild or restart to change a layout.

The base is the official stable release **1.28.1**, tag **v1.28.1**, commit
**e8bc16acee5ae1937600d571d441c4b5d7b0d288**. GitHub's official latest-release API
reported `draft: false` and `prerelease: false` when checked on November 18, 2026.
The release was published on November 11, 2026. The release source also contains `stable` in `crates/zed/RELEASE_CHANNEL`. The
development branch is `held-key-modifiers-v1.28.1`; no changes from upstream
`main` are included.

## Configuration {#configuration}

```json [keymap]
[
  {
    "context": "Editor && mode == full",
    "held_modifiers": {
      "ctrl-f": { "name": "word" },
      "ctrl-d": { "name": "select" }
    },
    "bindings": {
      "ctrl-l": "editor::MoveRight"
    }
  },
  {
    "context": "Editor && mode == full",
    "when_held": ["word"],
    "bindings": {
      "ctrl-l": "editor::MoveToNextWordEnd"
    }
  },
  {
    "context": "Editor && mode == full",
    "when_held": ["select"],
    "bindings": {
      "ctrl-l": "editor::SelectRight"
    }
  },
  {
    "context": "Editor && mode == full",
    "when_held": ["word", "select"],
    "bindings": {
      "ctrl-l": "editor::SelectToNextWordEnd"
    }
  }
]
```

`held_modifiers` maps a single keystroke to an object containing `name`. Names are
case-sensitive and start with an ASCII letter, followed by letters, digits, `_`,
or `-`. Names are arbitrary; there are no built-in `word` or `select` behaviors or
fixed limit of two modifiers. Multiple declarations can use the same name. The
name stays active while any corresponding press remains active. If several
applicable declarations match one keystroke, the last declaration wins.

Declarations use the existing keystroke parser and `use_key_equivalents` option.
Sequences such as `ctrl-f j`, standalone native modifiers such as `ctrl`, unknown
definition fields, invalid names, duplicate requirements, and undeclared names
produce configuration errors. A declaration cannot have `when_held` requirements.
Forward references to declarations elsewhere in the same file are allowed.
Invalid conditional sections are skipped rather than loaded unconditionally.

The declaration's context must match the focused context. An editor ancestor
behind a search input does not activate `Editor && mode == full`. Outside the
declaration's context the key keeps its normal behavior. Activation requires the
configured native modifiers exactly, like a normal shortcut. Additional native
modifiers pressed afterwards do not cancel it; releasing a required modifier does.

The declared key is consumed immediately. Its ordinary bindings, raw key listeners,
and multi-stroke prefixes do not run, even if they appear later in the keymap.
Release only changes state. There is no tap action, hold threshold, timeout,
deferred shortcut, or generated character. Native modifier keys must be held
before pressing the declared key. Auto-repeat never activates a new modifier.

`when_held` requires all listed names; extra held names are allowed. Order does not
matter. With the example above, hold Control and F, press L repeatedly, release F,
and press L again to return immediately to character movement. Add D to select.
Change `ctrl-f` to `ctrl-s` in JSON to move the word modifier to S.

## Precedence and fallback {#precedence}

Eligible action bindings are ranked by:

1. Existing context depth: the deepest matching context wins.
2. At equal depth, the number of names in `when_held`: more names win.
3. At equal depth and number of names, the later binding wins, as in upstream Zed.

An absent or empty `when_held` means zero requirements. If a key has no eligible
held binding, ordinary bindings still apply; otherwise normal text handling applies.
For example, holding `word` and pressing a key defined only in the base section
runs that base binding. Equal-size overlapping requirements are resolved by file
order. Existing `null`, `unbind`, source precedence, and action propagation remain
in effect. A conditional `null` suppresses bindings only while its requirements
are met; as in upstream, `null` is not a text-input blocker.

Held activation and deactivation cancel pending multi-stroke input without replay.
A lower-priority ordinary sequence cannot delay a more specific held binding.
Sequences explicitly configured inside a held section still use Zed's normal
sequence rules, including its existing timeout where applicable. Holding a
modifier itself never starts a sequence or timer.

## Complete navigation layout {#example}

The complete, valid JSON example is at
`assets/keymaps/held-modifiers.example.json` in the repository. Copy its sections
into your keymap array. It includes all requested navigation, word selection,
line and page movement, deletion, new-line insertion, and search shortcuts.
Existing Command shortcuts remain in the built-in keymap.

Line movement targets the real line beginning/end, not indentation or soft wraps.
Page movement uses `editor::MovePageUp` and `editor::MovePageDown`. Search movement
uses `search::SelectNextMatch` and `search::SelectPreviousMatch`, the same actions as
the built-in macOS next/previous match shortcuts. Word movement uses Zed's existing
word boundaries: previous word start and next word end.

Edit held declarations and conditional sections directly in JSON. Held declarations and conditional bindings are omitted from the graphical
keymap editor, which does not have controls for their conditions.

## Focus, reload, and input methods {#input-lifecycle}

Changing focus, switching windows, or losing application focus cancels active
names. Reloading bindings also cancels them, including keyboard-layout changes that trigger a keymap reload.
Press the modifier again to activate it in the new context. Returning focus or
pressing Control again does not revive an old press.

Cancelled presses remain consumed until release. Their repeats cannot insert text
even if Control was released first. A fresh non-repeat key-down clears any stale
record, so a missed key-up does not suppress future typing. Native modifier state
on subsequent events also cancels stale combinations. No local event system can
infer an unreported physical release in the absence of any further events; focus,
native-modifier, reload, and fresh-press events provide reconciliation points.

Keyboard-layout translation uses the existing GPUI matching code. IME composition
and AltGr character-input preference retain their upstream priority, so a key
consumed by an IME before GPUI sees it cannot activate a held modifier. On macOS,
a small native-event guard keeps repeats of already-consumed physical presses
away from the IME, including after Control is released. No global listeners,
extra permissions, polling, or remapping process are used.

## Build and test on macOS {#build}

Follow the [macOS prerequisites](./macos.md): Xcode, its command-line tools,
Rustup, and CMake. The release pins Rust 1.98.1 in `rust-toolchain.toml`. Set
`"auto_update": false` in the custom build's `settings.json` to avoid replacing it
with an official binary that lacks this feature. Upgrade the source deliberately
from a verified stable tag.

```sh
git switch held-key-modifiers-v1.28.1
cargo run -p zed
```

If Xcode's Metal compiler is unavailable, this release supports runtime shader
compilation without a source change:

```sh
cargo run -p zed --features gpui_platform/runtime_shaders
```

For an optimized build, add `--release`. For an app bundle, use `script/bundle-mac`
after installing the normal build prerequisites. Use a separate configuration
folder for a smoke test if you want to preserve your usual Zed configuration:

```sh
mkdir -p /tmp/zed-held-data/config
cp assets/keymaps/held-modifiers.example.json /tmp/zed-held-data/config/keymap.json
cargo run -p zed --features gpui_platform/runtime_shaders -- --user-data-dir /tmp/zed-held-data
```

Run the focused tests, then the existing GPUI and settings suites:

```sh
cargo test -p gpui --lib held --features gpui_platform/runtime_shaders
cargo test -p gpui_macos --lib held --features runtime_shaders
cargo test -p settings -p gpui_platform --lib held --features gpui_platform/runtime_shaders
cargo test -p search -p gpui_platform --lib held_modifiers_example --features gpui_platform/runtime_shaders
cargo test -p gpui --lib --features gpui_platform/runtime_shaders
cargo test -p settings -p gpui_platform --lib --features gpui_platform/runtime_shaders
```

On machines with limited disk space, add
`--config profile.dev.debug=0 --config profile.dev.incremental=false` to build and
test commands. Use the same flags consistently to reuse build artifacts.

For a manual smoke test, use a file containing `one two three` and verify:

- Character movement; repeated word movement; immediate return to characters.
- Selection and word selection, pressing the two held keys in either order.
- Modifier-only taps, OS repeat, and releasing Control before the ordinary keys.
- Ordinary typing, Command shortcuts, command palette, terminal, and search inputs.
- Focus changes between editors/windows and switching away from Zed while held.
- Editing F to S in the keymap while keys are pressed, then testing without restart.
- A non-QWERTY layout, dead keys, and an IME, including repeats after Control release.

## Validation on November 18, 2026 {#validation}

Validated on macOS 26.7.3 (Apple Silicon), using Rust 1.98.1 and runtime shaders:

| Check                                                         | Result                               |
| ------------------------------------------------------------- | ------------------------------------ |
| Full GPUI library suite                                       | 423 passed; 2 existing tests ignored |
| Full settings library suite                                   | 52 passed                            |
| macOS physical-key identity test                              | 1 passed                             |
| Complete example with real editor actions and reload          | 1 passed                             |
| Native Zed build                                              | Passed                               |
| Rust formatting; changed documentation and example formatting | Passed                               |

The application launched with a separate data/configuration folder. The native UI
smoke test could not be completed: the organization's Computer Use policy denied
access to the temporary test application (`dev.zed.held-modifiers-smoke`). The
manual checks above remain necessary, especially OS-generated repeat, native
focus transitions, non-QWERTY layouts, and IME composition. Automated tests cover
the corresponding event-state transitions, but do not replace physical-key tests.

## Components and upstream maintenance {#maintenance}

- `settings::KeymapFile` loads declarations and requirements, generates their JSON
  schema, and reports errors through the existing partial-load error UI.
- `gpui::KeyBinding` carries names and requirements. `Keymap` indexes declarations
  separately and adds requirement count to matching precedence.
- `gpui::HeldModifiers` keeps window-local active and cancelled state. An app-local consumed-key list
  suppresses repeats even when a press moves to another window. `Window`
  consumes presses before prefix matching and action dispatch; `App` cancels state
  when bindings change. `DispatchTree` forwards active names to the matcher.
- `gpui_macos::window` remembers physical keys consumed as modifiers so an IME
  cannot turn their later repeats into text. This only affects consumed presses.
- Tests exercise state transitions, precedence, dispatch conflicts, parser errors,
  reloads, and the complete example against real editor actions in the search crate.

When upgrading, start from another verified stable tag and port these changes.
Review GPUI event ordering, pending-input replay, context depth, keymap reload,
and macOS IME handling. Rerun tests and the physical-key smoke test, especially if
upstream changes key translation or modifier-only shortcuts. No editor movement
algorithm or built-in keymap is changed by this feature.

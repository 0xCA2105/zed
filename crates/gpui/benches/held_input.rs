//! Keyboard routing cost, including the production platform callback, focused
//! context lookup and action dispatch. This deliberately excludes editor layout,
//! text storage and native IME work; it does not measure end-to-end typing latency.
use gpui::{
    BenchAppContext, Context, FocusHandle, InteractiveElement, IntoElement, KeyBinding,
    KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, NoAction, ParentElement,
    PlatformInput, Render, Styled, Window, div,
};
use std::{cell::Cell, fmt, rc::Rc, time::Duration};

gpui::actions!(held_input_bench, [Move]);

struct InputView {
    focus: FocusHandle,
    context: &'static str,
    actions: Rc<Cell<usize>>,
}
impl Render for InputView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let actions = self.actions.clone();
        div().size_full().key_context("Workspace").child(
            div().size_full().key_context("Pane").child(
                div()
                    .size_full()
                    .key_context(self.context)
                    .track_focus(&self.focus)
                    .on_action(move |_: &Move, _, _| actions.set(actions.get() + 1)),
            ),
        )
    }
}
struct Case {
    declarations: usize,
    context: &'static str,
    active: bool,
}
impl fmt::Display for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}-{}",
            self.declarations,
            self.context.split(' ').next().unwrap(),
            if self.active { "active" } else { "inactive" }
        )
    }
}
fn cases() -> Vec<Case> {
    vec![
        Case {
            declarations: 0,
            context: "Editor mode=full",
            active: false,
        },
        Case {
            declarations: 2,
            context: "Editor mode=full",
            active: false,
        },
        Case {
            declarations: 2,
            context: "Terminal",
            active: false,
        },
        Case {
            declarations: 8,
            context: "Editor mode=full",
            active: false,
        },
        Case {
            declarations: 2,
            context: "Editor mode=full",
            active: true,
        },
    ]
}
fn down(source: &'static str) -> PlatformInput {
    PlatformInput::KeyDown(KeyDownEvent {
        keystroke: Keystroke::parse(source).unwrap(),
        is_held: false,
        prefer_character_input: false,
    })
}
fn up(source: &'static str) -> PlatformInput {
    PlatformInput::KeyUp(KeyUpEvent {
        keystroke: Keystroke::parse(source).unwrap(),
    })
}

#[gpui::bench(inputs = cases(), input_name = "case", group = "held_input", fps = 120)]
fn key_pair(case: &Case, cx: &mut BenchAppContext) {
    assert!(!cfg!(debug_assertions), "use --profile release-fast");
    cx.update(|cx| {
        // 64 ordinary bindings across typical app contexts, plus the held layer.
        for context in ["Workspace", "Pane", "Editor && mode == full", "Terminal"] {
            cx.bind_keys(
                [
                    "cmd-c",
                    "cmd-v",
                    "cmd-x",
                    "cmd-z",
                    "cmd-s",
                    "cmd-f",
                    "cmd-a",
                    "cmd-p",
                    "ctrl-a",
                    "ctrl-e",
                    "ctrl-n",
                    "ctrl-p",
                    "alt-left",
                    "alt-right",
                    "shift-left",
                    "shift-right",
                ]
                .map(|key| KeyBinding::new(key, NoAction, Some(context))),
            );
        }
        cx.bind_keys(
            [
                "ctrl-f", "ctrl-d", "ctrl-s", "ctrl-a", "ctrl-q", "ctrl-e", "ctrl-r", "ctrl-t",
            ]
            .into_iter()
            .take(case.declarations)
            .map(|key| {
                KeyBinding::new(key, NoAction, Some("Editor && mode == full"))
                    .with_held_modifier("word".into())
                    .unwrap()
            }),
        );
        if case.declarations > 0 {
            cx.bind_keys([
                KeyBinding::new("ctrl-l", Move, Some("Editor && mode == full"))
                    .with_when_held(["word".into()]),
            ]);
        }
    });
    let actions = Rc::new(Cell::new(0));
    let mut window = cx.add_empty_window();
    let _view = window.update(|window, cx| {
        window.replace_root(cx, |window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            InputView {
                focus,
                context: case.context,
                actions: actions.clone(),
            }
        })
    });
    cx.run_until_idle();
    if case.active {
        window.dispatch_input(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
            modifiers: Modifiers::control(),
            ..Default::default()
        }));
        assert!(window.dispatch_input(down("ctrl-f")));
    }
    let key = if case.active { "ctrl-l" } else { "z->z" };
    let key_down = down(key);
    let key_up = up(key);
    // Warm drawing, focus and input setup before measuring.
    assert_eq!(window.dispatch_input(key_down.clone()), case.active);
    assert!(!window.dispatch_input(key_up.clone()));
    cx.run_until_idle();
    let before = actions.get();
    let mut iterations = 0;
    cx.bench_iter(|_| {
        assert_eq!(window.dispatch_input(key_down.clone()), case.active);
        assert!(!window.dispatch_input(key_up.clone()));
        iterations += 1;
    });
    assert_eq!(
        actions.get() - before,
        if case.active { iterations } else { 0 }
    );
    if case.active {
        assert!(window.dispatch_input(up("ctrl-f")));
    }
}

gpui::bench_group! {
    name = benches;
    config = criterion::Criterion::default().sample_size(10)
        .warm_up_time(Duration::from_millis(300)).measurement_time(Duration::from_secs(1)).without_plots();
    targets = key_pair
}
gpui::bench_main!(benches);

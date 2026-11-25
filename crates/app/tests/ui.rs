//! UI integration tests: the real WorkspaceView in a headless window, driven
//! by native clicks and key presses.

use std::fs;
use std::path::Path;

mod support;

use agent_desk::app_state::AppState;
use agent_desk::workspace_view::{WorkspaceView, new_thread_button_id};
use agent_desk_core::store::{ProjectList, ThreadStore};
use agent_desk_core::thread::Item;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};
use support::test_approver;

fn open_workspace(root: &Path, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    cx.update(gpui_kit::init);
    cx.update(agent_desk::theme::install);
    cx.update(agent_desk::actions::bind_keys);
    let root = root.to_path_buf();
    let missing_claude = root.join("no-claude-here");
    let claude_home = root.join("claude-home");
    agent_desk_test_support::write_model_catalog(&claude_home);
    let app =
        cx.new(|_| AppState::load_with_history(root, Some(missing_claude), Some(claude_home)));
    let window_app = app.clone();
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1000.), px(700.)),
                ))),
                ..Default::default()
            },
            cx,
            move |window, cx| cx.new(|cx| WorkspaceView::new(window_app, None, window, cx)),
        )
        .expect("open test window")
    });
    (handle, app)
}

#[gpui_kit::test]
fn empty_workspace_offers_add_project(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let (handle, _) = open_workspace(root.path(), cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("add-project-empty").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn new_thread_then_enter_sends_the_message(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = work.path().join("repo");
    fs::create_dir(&project).unwrap();
    let project = ProjectList::load(root.path().into())
        .add(&project)
        .unwrap()
        .path;

    let (handle, app) = open_workspace(root.path(), cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(new_thread_button_id(&project), cx);

        // An empty draft cannot be sent.
        assert_eq!(window.find("send").disabled(), Some(true));

        // The new thread's composer has focus, so typing goes straight into it.
        window.input("fix the login bug", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();

    app.read_with(cx, |app, _| {
        let thread = app.selected_thread().expect("the new thread is selected");
        assert_eq!(thread.items, [Item::User("fix the login bug".into())]);
        assert_eq!(thread.title, "fix the login bug");
    });
}

#[gpui_kit::test]
fn new_thread_shortcut_creates_a_thread(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    fs::create_dir(work.path().join("repo")).unwrap();
    ProjectList::load(root.path().into())
        .add(&work.path().join("repo"))
        .unwrap();

    let (handle, app) = open_workspace(root.path(), cx);
    let shortcut = if cfg!(target_os = "macos") {
        "cmd-n"
    } else {
        "ctrl-n"
    };
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press(shortcut, cx);
    })
    .unwrap();
    cx.run_until_parked();

    app.read_with(cx, |app, _| {
        assert_eq!(app.threads().len(), 1);
        assert!(app.selected_thread().is_some());
    });
}

#[gpui_kit::test]
fn appearance_button_toggles_and_saves_the_theme(cx: &mut TestAppContext) {
    use agent_desk_core::settings::{Appearance, Settings};
    use gpui_kit::component::Theme;

    let root = tempfile::tempdir().unwrap();
    let (handle, app) = open_workspace(root.path(), cx);
    let is_dark = |cx: &mut TestAppContext| cx.update(|cx| Theme::global(cx).mode.is_dark());
    let started_dark = is_dark(cx);

    // One click switches mode: no menu to open.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("appearance", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(is_dark(cx), !started_dark);
    let saved = if started_dark {
        Appearance::Light
    } else {
        Appearance::Dark
    };
    assert_eq!(app.read_with(cx, |app, _| app.appearance()), saved);
    assert_eq!(Settings::load(root.path()).appearance, saved);

    // A second click switches back.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("appearance", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(is_dark(cx), started_dark);
}

#[gpui_kit::test]
fn stop_shortcut_ends_the_live_session(cx: &mut TestAppContext) {
    use agent_desk_test_support::fake_claude_path;

    cx.executor().allow_parking();
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    fs::create_dir(work.path().join("repo")).unwrap();
    let project = ProjectList::load(root.path().into())
        .add(&work.path().join("repo"))
        .unwrap()
        .path;

    cx.update(gpui_kit::init);
    cx.update(agent_desk::theme::install);
    cx.update(agent_desk::actions::bind_keys);
    let app = cx.new(|_| {
        AppState::load_with_history(root.path().into(), Some(fake_claude_path()), None)
            .with_approver(test_approver())
    });
    let window_app = app.clone();
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| WorkspaceView::new(window_app, None, window, cx))
        })
        .expect("open test window")
    });
    let id = app.update(cx, |app, cx| {
        let id = app.new_thread(&project, cx);
        app.send_message(&id, "hi".into(), cx);
        id
    });
    assert!(app.read_with(cx, |app, _| app.is_live(&id)));

    let shortcut = if cfg!(target_os = "macos") {
        "cmd-."
    } else {
        "ctrl-."
    };
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press(shortcut, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!app.read_with(cx, |app, _| app.is_live(&id)));
}

#[gpui_kit::test]
fn model_picker_shows_models_and_reasoning_side_by_side(cx: &mut TestAppContext) {
    use gpui_kit::MouseButton;
    use gpui_kit::test::{TestEventExt, TestQueryExt};

    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    fs::create_dir(work.path().join("repo")).unwrap();
    let project = ProjectList::load(root.path().into())
        .add(&work.path().join("repo"))
        .unwrap()
        .path;
    let (handle, app) = open_workspace(root.path(), cx);
    let id = app.update(cx, |app, cx| app.new_thread(&project, cx));

    let click = |window: &mut gpui_kit::Window, label: &str, cx: &mut gpui_kit::App| {
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    };

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // The picker sits on the right of the composer, beside Send.
        let picker = window.find("model-picker").bounds();
        let send = window.find_by_label("Send").bounds();
        assert!(picker.right() <= send.left(), "picker is left of Send");
        assert!(
            send.left() - picker.right() < gpui_kit::px(24.),
            "picker is next to Send"
        );
        window.click("model-picker", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let model = window.find_by_label("Opus 5.5").bounds();
        let level = window.find_by_label("High").bounds();
        assert!(
            level.left() >= model.right(),
            "reasoning is to the right of models"
        );
        assert!(
            level.top() < model.bottom() + gpui_kit::px(200.),
            "the columns share rows"
        );
        click(window, "Opus 5.5", cx);
    })
    .unwrap();
    cx.run_until_parked();

    // Still open: choose the reasoning level without reopening.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        click(window, "High", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let thread = app.read_with(cx, |app, _| {
        app.threads().iter().find(|t| t.id == id).unwrap().clone()
    });
    assert_eq!(thread.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(thread.effort.as_deref(), Some("high"));
}

#[gpui_kit::test]
fn mode_toggle_switches_between_supervised_and_full_access(cx: &mut TestAppContext) {
    use agent_desk_core::thread::PermissionMode;

    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    fs::create_dir(work.path().join("repo")).unwrap();
    let project = ProjectList::load(root.path().into())
        .add(&work.path().join("repo"))
        .unwrap()
        .path;
    let (handle, app) = open_workspace(root.path(), cx);
    let id = app.update(cx, |app, cx| app.new_thread(&project, cx));
    let mode = |cx: &mut TestAppContext| {
        app.read_with(cx, |app, _| {
            app.threads()
                .iter()
                .find(|t| t.id == id)
                .unwrap()
                .permission_mode
        })
    };
    assert_eq!(mode(cx), PermissionMode::Supervised);

    for expected in [PermissionMode::FullAccess, PermissionMode::Supervised] {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            // The toggle sits with the composer's other controls, before the picker.
            let toggle = window.find("permission-mode").bounds();
            let picker = window.find("model-picker").bounds();
            assert!(
                toggle.right() <= picker.left(),
                "toggle is left of the picker"
            );
            window.click("permission-mode", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(mode(cx), expected);
    }
}

#[gpui_kit::test]
fn a_compaction_divider_opens_to_its_summary_and_closes_again(cx: &mut TestAppContext) {
    use agent_desk_core::claude::AgentEvent;
    use agent_desk_core::thread::Thread;

    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = work.path().join("repo");
    fs::create_dir(&project).unwrap();
    let mut thread = Thread::new(project);
    for n in 0..30 {
        thread.push_user(&format!("question {n}"));
        thread.apply(&AgentEvent::AssistantText {
            text: format!("answer {n}"),
        });
    }
    // Last row of a thread that scrolls and sits at its end, so a row that
    // grows pushes the divider up.
    let marker_ix = thread.items.len();
    let marker = format!("compaction-{marker_ix}");
    thread.apply(&AgentEvent::Compacted {
        trigger: "auto".into(),
        pre_tokens: Some(100),
        post_tokens: Some(10),
    });
    thread.apply(&AgentEvent::CompactSummary {
        text: "What was kept".into(),
    });
    let id = thread.id.clone();
    ThreadStore::new(root.path().into()).save(&thread).unwrap();

    let (handle, app) = open_workspace(root.path(), cx);
    app.update(cx, |app, cx| app.select(&id, cx));
    // A click's effects settle outside the window update, and a row whose
    // height changed is measured on one frame and drawn on the next. So each
    // step is click, settle, draw twice.
    let divider_top = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            window.find(marker.clone()).bounds().origin.y
        })
        .unwrap()
    };
    let click_divider = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| window.click(marker.clone(), cx))
            .unwrap();
    };

    // The thread is anchored to the bottom, so a row that grows (the summary
    // opening under the divider) pushes the divider up.
    let closed = divider_top(cx);
    click_divider(cx);
    let open = divider_top(cx);
    assert!(
        open < closed,
        "opening the summary makes room: {open:?} vs {closed:?}"
    );
    click_divider(cx);
    assert_eq!(divider_top(cx), closed, "closing it gives the room back");
}

#[gpui_kit::test]
fn stop_takes_the_place_of_send_while_claude_works(cx: &mut TestAppContext) {
    use agent_desk_core::thread::ThreadStatus;
    use agent_desk_test_support::fake_claude_path;

    cx.executor().allow_parking();
    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    fs::create_dir(work.path().join("repo")).unwrap();
    let project = ProjectList::load(root.path().into())
        .add(&work.path().join("repo"))
        .unwrap()
        .path;
    cx.update(gpui_kit::init);
    cx.update(agent_desk::theme::install);
    cx.update(agent_desk::actions::bind_keys);
    let app = cx.new(|_| {
        AppState::load_with_history(root.path().into(), Some(fake_claude_path()), None)
            .with_approver(test_approver())
    });
    let window_app = app.clone();
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| WorkspaceView::new(window_app, None, window, cx))
        })
        .expect("open test window")
    });
    // The stand-in takes three seconds over this reply; Stop ends it sooner.
    let id = app.update(cx, |app, cx| {
        let id = app.new_thread(&project, cx);
        app.send_message(&id, "slow 3".into(), cx);
        id
    });

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("stop-thread").visible());
        assert!(window.try_find("send").is_none(), "Stop replaces Send");
        assert_eq!(window.find_all("stop-thread").len(), 1, "and only once");
        window.click("stop-thread", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let thread = app.read_with(cx, |app, _| {
        app.threads().iter().find(|t| t.id == id).unwrap().clone()
    });
    assert_eq!(thread.status, ThreadStatus::Failed("Stopped".into()));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("stop-thread").is_none());
        assert!(window.find("send").visible(), "Send comes back");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_long_thread_offers_a_jump_to_the_latest_message_when_scrolled_up(cx: &mut TestAppContext) {
    use std::time::Duration;

    use agent_desk_core::claude::AgentEvent;
    use agent_desk_core::thread::{Thread, ThreadStatus};
    use gpui_kit::{ElementId, ScrollDelta, point, px};

    let root = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = work.path().join("repo");
    fs::create_dir(&project).unwrap();
    let mut thread = Thread::new(project);
    for n in 0..40 {
        thread.push_user(&format!("question {n}"));
        thread.apply(&AgentEvent::AssistantText {
            text: format!("answer {n}"),
        });
    }
    // The last row has a button to scroll over: the harness finds only
    // interactive elements.
    let last_row = format!("compaction-{}", thread.items.len());
    thread.apply(&AgentEvent::Compacted {
        trigger: "auto".into(),
        pre_tokens: None,
        post_tokens: None,
    });
    thread.apply(&AgentEvent::CompactSummary {
        text: "kept".into(),
    });
    thread.status = ThreadStatus::Idle;
    let id = thread.id.clone();
    ThreadStore::new(root.path().into()).save(&thread).unwrap();

    let (handle, app) = open_workspace(root.path(), cx);
    app.update(cx, |app, cx| app.select(&id, cx));
    let jump = (ElementId::from("thread"), "jump-to-latest");

    // Draws settled frames; the button fades in and out over 200 ms.
    let draw = |cx: &mut TestAppContext| {
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        for _ in 0..3 {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
    };
    let has_jump_button = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, _| {
            window.try_find(jump.clone()).is_some()
        })
        .unwrap()
    };

    draw(cx);
    assert!(!has_jump_button(cx), "a thread opens at its latest message");

    cx.update_window(handle, |_, window, cx| {
        window.scroll(
            last_row.clone(),
            ScrollDelta::Pixels(point(px(0.), px(2000.))),
            cx,
        )
    })
    .unwrap();
    draw(cx);
    assert!(has_jump_button(cx), "scrolled up, the way back is offered");

    cx.update_window(handle, |_, window, cx| window.click(jump.clone(), cx))
        .unwrap();
    draw(cx);
    assert!(
        !has_jump_button(cx),
        "jumping returns to the latest message"
    );
}

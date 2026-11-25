//! UI integration tests: the real WorkspaceView in a headless window, driven
//! by native clicks and key presses.

use std::fs;
use std::path::Path;

use agent_desk::app_state::AppState;
use agent_desk::workspace_view::{WorkspaceView, new_thread_button_id};
use agent_desk_core::store::ProjectList;
use agent_desk_core::thread::Item;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};

fn open_workspace(root: &Path, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AppState>) {
    cx.update(gpui_kit::init);
    cx.update(agent_desk::theme::install);
    cx.update(agent_desk::actions::bind_keys);
    let root = root.to_path_buf();
    let missing_claude = root.join("no-claude-here");
    let app = cx.new(|_| AppState::load_with_history(root, Some(missing_claude), None));
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
fn appearance_menu_switches_and_saves_the_theme(cx: &mut TestAppContext) {
    use agent_desk_core::settings::{Appearance, Settings};
    use gpui_kit::MouseButton;
    use gpui_kit::component::Theme;
    use gpui_kit::test::{TestEventExt, TestQueryExt};

    let root = tempfile::tempdir().unwrap();
    let (handle, app) = open_workspace(root.path(), cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("appearance", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let dark = window.find_by_label("Dark").bounds().center();
        window.pointer_down(dark, MouseButton::Left, cx);
        window.pointer_up(dark, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    assert_eq!(
        app.read_with(cx, |app, _| app.appearance()),
        Appearance::Dark
    );
    assert!(cx.update(|cx| Theme::global(cx).mode.is_dark()));
    assert_eq!(Settings::load(root.path()).appearance, Appearance::Dark);
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
    let app =
        cx.new(|_| AppState::load_with_history(root.path().into(), Some(fake_claude_path()), None));
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
fn model_menu_sets_the_threads_model(cx: &mut TestAppContext) {
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

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // The icon-only Send button still has a name for assistive technology.
        assert!(window.try_find_by_label("Send").is_some());
        window.click("model-picker", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let opus = window.find_by_label("Opus").bounds().center();
        window.pointer_down(opus, MouseButton::Left, cx);
        window.pointer_up(opus, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let model = app.read_with(cx, |app, _| {
        app.threads()
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .model
            .clone()
    });
    assert_eq!(model.as_deref(), Some("opus"));
}

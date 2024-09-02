mod app_menus;

pub use app_menus::*;
use gpui::{
    actions, point, px, AppContext, MenuItem, PromptLevel, TitlebarOptions, WindowKind,
    WindowOptions,
};

use futures::{channel::mpsc, select_biased, StreamExt};
use release_channel::ReleaseChannel;
use settings::{KeymapFile, Settings, SettingsStore, DEFAULT_KEYMAP_PATH};
use theme::ActiveTheme;
use workspace::CloseIntent;

use util::ResultExt;
use uuid::Uuid;
use vim::VimModeSetting;
use welcome::BaseKeymap;
use workspace::{Workspace, WorkspaceSettings};
use zed_actions::Quit;

actions!(
    zed,
    [
        DebugElements,
        Hide,
        HideOthers,
        Minimize,
        OpenDefaultKeymap,
        OpenDefaultSettings,
        OpenLocalSettings,
        OpenLocalTasks,
        OpenTasks,
        ResetDatabase,
        ShowAll,
        ToggleFullScreen,
        Zoom,
        TestPanic,
    ]
);

pub fn init(cx: &mut AppContext) {
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &Hide, cx| cx.hide());
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(quit);

    if ReleaseChannel::global(cx) == ReleaseChannel::Dev {
        cx.on_action(test_panic);
    }
}

pub fn build_window_options(display_uuid: Option<Uuid>, cx: &mut AppContext) -> WindowOptions {
    let display = display_uuid.and_then(|uuid| {
        cx.displays()
            .into_iter()
            .find(|display| display.uuid().ok() == Some(uuid))
    });
    let app_id = ReleaseChannel::global(cx).app_id();
    let window_decorations = match std::env::var("ZED_WINDOW_DECORATIONS") {
        Ok(val) if val == "server" => gpui::WindowDecorations::Server,
        Ok(val) if val == "client" => gpui::WindowDecorations::Client,
        _ => gpui::WindowDecorations::Client,
    };

    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.0), px(9.0))),
        }),
        window_bounds: None,
        focus: false,
        show: false,
        kind: WindowKind::Normal,
        is_movable: true,
        display_id: display.map(|display| display.id()),
        window_background: cx.theme().window_background_appearance(),
        app_id: Some(app_id.to_owned()),
        window_decorations: Some(window_decorations),
        window_min_size: Some(gpui::Size {
            width: px(360.0),
            height: px(240.0),
        }),
    }
}

fn test_panic(_: &TestPanic, _: &mut AppContext) {
    panic!("Ran the TestPanic action")
}

fn quit(_: &Quit, cx: &mut AppContext) {
    let should_confirm = WorkspaceSettings::get_global(cx).confirm_quit;
    cx.spawn(|mut cx| async move {
        let mut workspace_windows = cx.update(|cx| {
            cx.windows()
                .into_iter()
                .filter_map(|window| window.downcast::<Workspace>())
                .collect::<Vec<_>>()
        })?;

        // If multiple windows have unsaved changes, and need a save prompt,
        // prompt in the active window before switching to a different window.
        cx.update(|mut cx| {
            workspace_windows.sort_by_key(|window| window.is_active(&mut cx) == Some(false));
        })
        .log_err();

        if let (true, Some(workspace)) = (should_confirm, workspace_windows.first().copied()) {
            let answer = workspace
                .update(&mut cx, |_, cx| {
                    cx.prompt(
                        PromptLevel::Info,
                        "Are you sure you want to quit?",
                        None,
                        &["Quit", "Cancel"],
                    )
                })
                .log_err();

            if let Some(answer) = answer {
                let answer = answer.await.ok();
                if answer != Some(0) {
                    return Ok(());
                }
            }
        }

        // If the user cancels any save prompt, then keep the app open.
        for window in workspace_windows {
            if let Some(should_close) = window
                .update(&mut cx, |workspace, cx| {
                    workspace.prepare_to_close(CloseIntent::Quit, cx)
                })
                .log_err()
            {
                if !should_close.await? {
                    return Ok(());
                }
            }
        }
        cx.update(|cx| cx.quit())?;
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

pub fn handle_keymap_file_changes(
    mut user_keymap_file_rx: mpsc::UnboundedReceiver<String>,
    cx: &mut AppContext,
    keymap_changed: impl Fn(Option<anyhow::Error>, &mut AppContext) + 'static,
) {
    BaseKeymap::register(cx);
    VimModeSetting::register(cx);

    let (base_keymap_tx, mut base_keymap_rx) = mpsc::unbounded();
    let mut old_base_keymap = *BaseKeymap::get_global(cx);
    let mut old_vim_enabled = VimModeSetting::get_global(cx).0;
    cx.observe_global::<SettingsStore>(move |cx| {
        let new_base_keymap = *BaseKeymap::get_global(cx);
        let new_vim_enabled = VimModeSetting::get_global(cx).0;

        if new_base_keymap != old_base_keymap || new_vim_enabled != old_vim_enabled {
            old_base_keymap = new_base_keymap;
            old_vim_enabled = new_vim_enabled;
            base_keymap_tx.unbounded_send(()).unwrap();
        }
    })
    .detach();

    load_default_keymap(cx);

    cx.spawn(move |cx| async move {
        let mut user_keymap = KeymapFile::default();
        loop {
            select_biased! {
                _ = base_keymap_rx.next() => {}
                user_keymap_content = user_keymap_file_rx.next() => {
                    if let Some(user_keymap_content) = user_keymap_content {
                        match KeymapFile::parse(&user_keymap_content) {
                            Ok(keymap_content) => {
                                cx.update(|cx| keymap_changed(None, cx)).log_err();
                                user_keymap = keymap_content;
                            }
                            Err(error) => {
                                cx.update(|cx| keymap_changed(Some(error), cx)).log_err();
                            }
                        }
                    }
                }
            }
            cx.update(|cx| reload_keymaps(cx, &user_keymap)).ok();
        }
    })
    .detach();
}

fn reload_keymaps(cx: &mut AppContext, keymap_content: &KeymapFile) {
    cx.clear_key_bindings();
    load_default_keymap(cx);
    keymap_content.clone().add_to_cx(cx).log_err();
    cx.set_menus(app_menus());
    cx.set_dock_menu(vec![MenuItem::action("New Window", workspace::NewWindow)])
}

pub fn load_default_keymap(cx: &mut AppContext) {
    let base_keymap = *BaseKeymap::get_global(cx);
    if base_keymap == BaseKeymap::None {
        return;
    }

    KeymapFile::load_asset(DEFAULT_KEYMAP_PATH, cx).unwrap();
    if VimModeSetting::get_global(cx).0 {
        KeymapFile::load_asset("keymaps/vim.json", cx).unwrap();
    }

    if let Some(asset_path) = base_keymap.asset_path() {
        KeymapFile::load_asset(asset_path, cx).unwrap();
    }
}

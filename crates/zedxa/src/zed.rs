mod app_menus;

pub use app_menus::*;
use breadcrumbs::Breadcrumbs;
use client::ZED_URL_SCHEME;
use collections::VecDeque;
use editor::{scroll::Autoscroll, Editor, MultiBuffer};
use gpui::{
    actions, point, px, AppContext, AsyncAppContext, Context, FocusableView, MenuItem, PromptLevel,
    TitlebarOptions, View, ViewContext, VisualContext, WindowKind, WindowOptions,
};

use anyhow::Context as _;
use futures::{channel::mpsc, select_biased, StreamExt};
use outline_panel::OutlinePanel;
use project::TaskSourceKind;
use project_panel::ProjectPanel;
use release_channel::{AppCommitSha, ReleaseChannel};
use search::project_search::ProjectSearchBar;
use settings::{
    initial_local_settings_content, initial_tasks_content, watch_config_file, KeymapFile, Settings,
    SettingsStore, DEFAULT_KEYMAP_PATH,
};
use std::{borrow::Cow, ops::Deref, path::Path, sync::Arc};
use task::static_source::{StaticSource, TrackedFile};
use theme::ActiveTheme;
use workspace::notifications::NotificationId;
use workspace::CloseIntent;

use paths::{local_settings_file_relative_path, local_tasks_file_relative_path};
use terminal_view::terminal_panel::{self, TerminalPanel};
use util::ResultExt;
use uuid::Uuid;
use vim::VimModeSetting;
use welcome::{BaseKeymap, MultibufferHint};
use workspace::{
    notifications::simple_message_notification::MessageNotification, open_new, AppState, NewFile,
    NewWindow, OpenLog, Toast, Workspace, WorkspaceSettings,
};
use workspace::{notifications::DetachAndPromptErr, Pane};
use zed_actions::{OpenAccountSettings, OpenBrowser, OpenSettings, Quit};

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

pub fn initialize_workspace(app_state: Arc<AppState>, cx: &mut AppContext) {
    cx.observe_new_views(move |workspace: &mut Workspace, cx| {
        let workspace_handle = cx.view().clone();
        let center_pane = workspace.active_pane().clone();
        initialize_pane(workspace, &center_pane, cx);
        cx.subscribe(&workspace_handle, {
            move |workspace, _, event, cx| match event {
                workspace::Event::PaneAdded(pane) => {
                    initialize_pane(workspace, pane, cx);
                }
                _ => {}
            }
        })
        .detach();

        if let Some(specs) = cx.gpu_specs() {
            log::info!("Using GPU: {:?}", specs);
            if specs.is_software_emulated && std::env::var("ZED_ALLOW_EMULATED_GPU").is_err() {
            let message = format!(db::indoc!{r#"
                Zed uses Vulkan for rendering and requires a compatible GPU.

                Currently you are using a software emulated GPU ({}) which
                will result in awful performance.

                For troubleshooting see: https://zed.dev/docs/linux
                "#}, specs.device_name);
            let prompt = cx.prompt(PromptLevel::Critical, "Unsupported GPU", Some(&message),
                &["Skip", "Troubleshoot and Quit"]);
            cx.spawn(|_, mut cx| async move {
                if prompt.await == Ok(1) {
                    cx.update(|cx| {
                        cx.open_url("https://zed.dev/docs/linux#zed-fails-to-open-windows");
                        cx.quit();
                    }).ok();
                }
            }).detach()
            }
        }

        let inline_completion_button = cx.new_view(|cx| {
            inline_completion_button::InlineCompletionButton::new(app_state.fs.clone(), cx)
        });

        let diagnostic_summary =
            cx.new_view(|cx| diagnostics::items::DiagnosticIndicator::new(workspace, cx));
        let activity_indicator =
            activity_indicator::ActivityIndicator::new(workspace, app_state.languages.clone(), cx);
        let active_buffer_language =
            cx.new_view(|_| language_selector::ActiveBufferLanguage::new(workspace));
        let vim_mode_indicator = cx.new_view(|cx| vim::ModeIndicator::new(cx));
        let cursor_position =
            cx.new_view(|_| go_to_line::cursor_position::CursorPosition::new(workspace));
        workspace.status_bar().update(cx, |status_bar, cx| {
            status_bar.add_left_item(diagnostic_summary, cx);
            status_bar.add_left_item(activity_indicator, cx);
            status_bar.add_right_item(inline_completion_button, cx);
            status_bar.add_right_item(active_buffer_language, cx);
            status_bar.add_right_item(vim_mode_indicator, cx);
            status_bar.add_right_item(cursor_position, cx);
        });

        auto_update::notify_of_any_new_update(cx);

        let handle = cx.view().downgrade();
        cx.on_window_should_close(move |cx| {
            handle
                .update(cx, |workspace, cx| {
                    // We'll handle closing asynchronously
                    workspace.close_window(&Default::default(), cx);
                    false
                })
                .unwrap_or(true)
        });

        let project = workspace.project().clone();
        if project.update(cx, |project, cx| {
            project.is_local_or_ssh() || project.ssh_connection_string(cx).is_some()
        }) {
            project.update(cx, |project, cx| {
                let fs = app_state.fs.clone();
                project.task_inventory().update(cx, |inventory, cx| {
                    let tasks_file_rx =
                        watch_config_file(&cx.background_executor(), fs, paths::tasks_file().clone());
                    inventory.add_source(
                        TaskSourceKind::AbsPath {
                            id_base: "global_tasks".into(),
                            abs_path: paths::tasks_file().clone(),
                        },
                        |tx, cx| StaticSource::new(TrackedFile::new(tasks_file_rx, tx, cx)),
                        cx,
                    );
                })
            });
        }

        cx.spawn(|workspace_handle, mut cx| async move {
            let project_panel = ProjectPanel::load(workspace_handle.clone(), cx.clone());
            let outline_panel = OutlinePanel::load(workspace_handle.clone(), cx.clone());
            let terminal_panel = TerminalPanel::load(workspace_handle.clone(), cx.clone());
            let (
                project_panel,
                outline_panel,
                terminal_panel,
            ) = futures::try_join!(
                project_panel,
                outline_panel,
                terminal_panel,
            )?;

            workspace_handle.update(&mut cx, |workspace, cx| {
                workspace.add_panel(project_panel, cx);
                workspace.add_panel(outline_panel, cx);
                workspace.add_panel(terminal_panel, cx);
                cx.focus_self();
            })
        })
        .detach();

        workspace
            .register_action(about)
            .register_action(|_, _: &Minimize, cx| {
                cx.minimize_window();
            })
            .register_action(|_, _: &Zoom, cx| {
                cx.zoom_window();
            })
            .register_action(|_, _: &ToggleFullScreen, cx| {
                cx.toggle_fullscreen();
            })
            .register_action(|_, action: &OpenBrowser, cx| cx.open_url(&action.url))
            .register_action(move |_, _: &zed_actions::IncreaseBufferFontSize, cx| {
                theme::adjust_buffer_font_size(cx, |size| *size += px(1.0))
            })
            .register_action(move |_, _: &zed_actions::DecreaseBufferFontSize, cx| {
                theme::adjust_buffer_font_size(cx, |size| *size -= px(1.0))
            })
            .register_action(move |_, _: &zed_actions::ResetBufferFontSize, cx| {
                theme::reset_buffer_font_size(cx)
            })
            .register_action(move |_, _: &zed_actions::IncreaseUiFontSize, cx| {
                theme::adjust_ui_font_size(cx, |size| *size += px(1.0))
            })
            .register_action(move |_, _: &zed_actions::DecreaseUiFontSize, cx| {
                theme::adjust_ui_font_size(cx, |size| *size -= px(1.0))
            })
            .register_action(move |_, _: &zed_actions::ResetUiFontSize, cx| {
                theme::reset_ui_font_size(cx)
            })
            .register_action(move |_, _: &zed_actions::IncreaseBufferFontSize, cx| {
                theme::adjust_buffer_font_size(cx, |size| *size += px(1.0))
            })
            .register_action(move |_, _: &zed_actions::DecreaseBufferFontSize, cx| {
                theme::adjust_buffer_font_size(cx, |size| *size -= px(1.0))
            })
            .register_action(move |_, _: &zed_actions::ResetBufferFontSize, cx| {
                theme::reset_buffer_font_size(cx)
            })
            .register_action(|_, _: &install_cli::Install, cx| {
                cx.spawn(|workspace, mut cx| async move {
                    if cfg!(target_os = "linux") {
                        let prompt = cx.prompt(
                            PromptLevel::Warning,
                            "CLI should already be installed",
                            Some("If you installed Zed from our official release add ~/.local/bin to your PATH.\n\nIf you installed Zed from a different source like your package manager, then you may need to create an alias/symlink manually.\n\nDepending on your package manager, the CLI might be named zeditor, zedit, zed-editor or something else."),
                            &["Ok"],
                        );
                        cx.background_executor().spawn(prompt).detach();
                        return Ok(());
                    }
                    let path = install_cli::install_cli(cx.deref())
                        .await
                        .context("error creating CLI symlink")?;

                    workspace.update(&mut cx, |workspace, cx| {
                        struct InstalledZedCli;

                        workspace.show_toast(
                            Toast::new(
                                NotificationId::unique::<InstalledZedCli>(),
                                format!(
                                    "Installed `zed` to {}. You can launch {} from your terminal.",
                                    path.to_string_lossy(),
                                    ReleaseChannel::global(cx).display_name()
                                ),
                            ),
                            cx,
                        )
                    })?;
                    register_zed_scheme(&cx).await.log_err();
                    Ok(())
                })
                .detach_and_prompt_err("Error installing zed cli", cx, |_, _| None);
            })
            .register_action(|_, _: &install_cli::RegisterZedScheme, cx| {
                cx.spawn(|workspace, mut cx| async move {
                    register_zed_scheme(&cx).await?;
                    workspace.update(&mut cx, |workspace, cx| {
                        struct RegisterZedScheme;

                        workspace.show_toast(
                            Toast::new(
                                NotificationId::unique::<RegisterZedScheme>(),
                                format!(
                                    "zed:// links will now open in {}.",
                                    ReleaseChannel::global(cx).display_name()
                                ),
                            ),
                            cx,
                        )
                    })?;
                    Ok(())
                })
                .detach_and_prompt_err(
                    "Error registering zed:// scheme",
                    cx,
                    |_, _| None,
                );
            })
            .register_action(|workspace, _: &OpenLog, cx| {
                open_log_file(workspace, cx);
            })
            .register_action(
                |_: &mut Workspace, _: &OpenAccountSettings, cx: &mut ViewContext<Workspace>| {
                    let server_url = &client::ClientSettings::get_global(cx).server_url;
                    cx.open_url(&format!("{server_url}/account"));
                },
            )
            .register_action(open_local_settings_file)
            .register_action(open_local_tasks_file)
            .register_action(
                |workspace: &mut Workspace,
                 _: &project_panel::ToggleFocus,
                 cx: &mut ViewContext<Workspace>| {
                    workspace.toggle_panel_focus::<ProjectPanel>(cx);
                },
            )
            .register_action(
                |workspace: &mut Workspace,
                 _: &outline_panel::ToggleFocus,
                 cx: &mut ViewContext<Workspace>| {
                    workspace.toggle_panel_focus::<OutlinePanel>(cx);
                },
            )
            .register_action(
                |workspace: &mut Workspace,
                 _: &terminal_panel::ToggleFocus,
                 cx: &mut ViewContext<Workspace>| {
                    workspace.toggle_panel_focus::<TerminalPanel>(cx);
                },
            )
            .register_action({
                let app_state = Arc::downgrade(&app_state);
                move |_, _: &NewWindow, cx| {
                    if let Some(app_state) = app_state.upgrade() {
                        open_new(app_state, cx, |workspace, cx| {
                            Editor::new_file(workspace, &Default::default(), cx)
                        })
                        .detach();
                    }
                }
            })
            .register_action({
                let app_state = Arc::downgrade(&app_state);
                move |_, _: &NewFile, cx| {
                    if let Some(app_state) = app_state.upgrade() {
                        open_new(app_state, cx, |workspace, cx| {
                            Editor::new_file(workspace, &Default::default(), cx)
                        })
                        .detach();
                    }
                }
            });

        workspace.focus_handle(cx).focus(cx);
    })
    .detach();
}

fn initialize_pane(_workspace: &mut Workspace, pane: &View<Pane>, cx: &mut ViewContext<Workspace>) {
    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            let multibuffer_hint = cx.new_view(|_| MultibufferHint::new());
            toolbar.add_item(multibuffer_hint, cx);
            let breadcrumbs = cx.new_view(|_| Breadcrumbs::new());
            toolbar.add_item(breadcrumbs, cx);
            let buffer_search_bar = cx.new_view(search::BufferSearchBar::new);
            toolbar.add_item(buffer_search_bar.clone(), cx);
            /*
            let quick_action_bar =
                cx.new_view(|cx| QuickActionBar::new(buffer_search_bar, workspace, cx));
            toolbar.add_item(quick_action_bar, cx);
            */
            let diagnostic_editor_controls = cx.new_view(|_| diagnostics::ToolbarControls::new());
            toolbar.add_item(diagnostic_editor_controls, cx);
            let project_search_bar = cx.new_view(|_| ProjectSearchBar::new());
            toolbar.add_item(project_search_bar, cx);
            let lsp_log_item = cx.new_view(|_| language_tools::LspLogToolbarItemView::new());
            toolbar.add_item(lsp_log_item, cx);
            let syntax_tree_item =
                cx.new_view(|_| language_tools::SyntaxTreeToolbarItemView::new());
            toolbar.add_item(syntax_tree_item, cx);
        })
    });
}

fn about(_: &mut Workspace, _: &zed_actions::About, cx: &mut gpui::ViewContext<Workspace>) {
    let release_channel = ReleaseChannel::global(cx).display_name();
    let version = env!("CARGO_PKG_VERSION");
    let message = format!("{release_channel} {version}");
    let detail = AppCommitSha::try_global(cx).map(|sha| sha.0.clone());

    let prompt = cx.prompt(PromptLevel::Info, &message, detail.as_deref(), &["OK"]);
    cx.foreground_executor()
        .spawn(async {
            prompt.await.ok();
        })
        .detach();
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

fn open_log_file(workspace: &mut Workspace, cx: &mut ViewContext<Workspace>) {
    const MAX_LINES: usize = 1000;
    workspace
        .with_local_workspace(cx, move |workspace, cx| {
            let fs = workspace.app_state().fs.clone();
            cx.spawn(|workspace, mut cx| async move {
                let (old_log, new_log) =
                    futures::join!(fs.load(paths::old_log_file()), fs.load(paths::log_file()));
                let log = match (old_log, new_log) {
                    (Err(_), Err(_)) => None,
                    (old_log, new_log) => {
                        let mut lines = VecDeque::with_capacity(MAX_LINES);
                        for line in old_log
                            .iter()
                            .flat_map(|log| log.lines())
                            .chain(new_log.iter().flat_map(|log| log.lines()))
                        {
                            if lines.len() == MAX_LINES {
                                lines.pop_front();
                            }
                            lines.push_back(line);
                        }
                        Some(
                            lines
                                .into_iter()
                                .flat_map(|line| [line, "\n"])
                                .collect::<String>(),
                        )
                    }
                };

                workspace
                    .update(&mut cx, |workspace, cx| {
                        let Some(log) = log else {
                            struct OpenLogError;

                            workspace.show_notification(
                                NotificationId::unique::<OpenLogError>(),
                                cx,
                                |cx| {
                                    cx.new_view(|_| {
                                        MessageNotification::new(format!(
                                            "Unable to access/open log file at path {:?}",
                                            paths::log_file().as_path()
                                        ))
                                    })
                                },
                            );
                            return;
                        };
                        let project = workspace.project().clone();
                        let buffer = project.update(cx, |project, cx| {
                            project.create_local_buffer(&log, None, cx)
                        });

                        let buffer = cx.new_model(|cx| {
                            MultiBuffer::singleton(buffer, cx).with_title("Log".into())
                        });
                        let editor = cx.new_view(|cx| {
                            let mut editor =
                                Editor::for_multibuffer(buffer, Some(project), true, cx);
                            editor.set_breadcrumb_header(format!(
                                "Last {} lines in {}",
                                MAX_LINES,
                                paths::log_file().display()
                            ));
                            editor
                        });

                        editor.update(cx, |editor, cx| {
                            let last_multi_buffer_offset = editor.buffer().read(cx).len(cx);
                            editor.change_selections(Some(Autoscroll::fit()), cx, |s| {
                                s.select_ranges(Some(
                                    last_multi_buffer_offset..last_multi_buffer_offset,
                                ));
                            })
                        });

                        workspace.add_item_to_active_pane(Box::new(editor), None, true, cx);
                    })
                    .log_err();
            })
            .detach();
        })
        .detach();
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

fn open_local_settings_file(
    workspace: &mut Workspace,
    _: &OpenLocalSettings,
    cx: &mut ViewContext<Workspace>,
) {
    open_local_file(
        workspace,
        local_settings_file_relative_path(),
        initial_local_settings_content(),
        cx,
    )
}

fn open_local_tasks_file(
    workspace: &mut Workspace,
    _: &OpenLocalTasks,
    cx: &mut ViewContext<Workspace>,
) {
    open_local_file(
        workspace,
        local_tasks_file_relative_path(),
        initial_tasks_content(),
        cx,
    )
}

fn open_local_file(
    workspace: &mut Workspace,
    settings_relative_path: &'static Path,
    initial_contents: Cow<'static, str>,
    cx: &mut ViewContext<Workspace>,
) {
    let project = workspace.project().clone();
    let worktree = project
        .read(cx)
        .visible_worktrees(cx)
        .find_map(|tree| tree.read(cx).root_entry()?.is_dir().then_some(tree));
    if let Some(worktree) = worktree {
        let tree_id = worktree.read(cx).id();
        cx.spawn(|workspace, mut cx| async move {
            if let Some(dir_path) = settings_relative_path.parent() {
                if worktree.update(&mut cx, |tree, _| tree.entry_for_path(dir_path).is_none())? {
                    project
                        .update(&mut cx, |project, cx| {
                            project.create_entry((tree_id, dir_path), true, cx)
                        })?
                        .await
                        .context("worktree was removed")?;
                }
            }

            if worktree.update(&mut cx, |tree, _| {
                tree.entry_for_path(settings_relative_path).is_none()
            })? {
                project
                    .update(&mut cx, |project, cx| {
                        project.create_entry((tree_id, settings_relative_path), false, cx)
                    })?
                    .await
                    .context("worktree was removed")?;
            }

            let editor = workspace
                .update(&mut cx, |workspace, cx| {
                    workspace.open_path((tree_id, settings_relative_path), None, true, cx)
                })?
                .await?
                .downcast::<Editor>()
                .context("unexpected item type: expected editor item")?;

            editor
                .downgrade()
                .update(&mut cx, |editor, cx| {
                    if let Some(buffer) = editor.buffer().read(cx).as_singleton() {
                        if buffer.read(cx).is_empty() {
                            buffer.update(cx, |buffer, cx| {
                                buffer.edit([(0..0, initial_contents)], None, cx)
                            });
                        }
                    }
                })
                .ok();

            anyhow::Ok(())
        })
        .detach();
    } else {
        struct NoOpenFolders;

        workspace.show_notification(NotificationId::unique::<NoOpenFolders>(), cx, |cx| {
            cx.new_view(|_| MessageNotification::new("This project has no folders open."))
        })
    }
}

async fn register_zed_scheme(cx: &AsyncAppContext) -> anyhow::Result<()> {
    cx.update(|cx| cx.register_url_scheme(ZED_URL_SCHEME))?
        .await
}

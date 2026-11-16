use super::super::*;
use gpui::{Modifiers, TestAppContext, VisualTestContext};

fn open_workspace(
    count: usize,
    cx: &mut TestAppContext,
) -> (tempfile::TempDir, Entity<NebulaWorkspace>, VisualTestContext) {
    let directory = tempfile::tempdir().unwrap();
    let hub = crate::runtime_api::RuntimeHub::new();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::gpui_shell::config::Settings::load(nebula_settings::ThemeName::Nord));
        crate::gpui_shell::math_view::register(cx);
        crate::gpui_shell::file_editor::init(cx);
        init(cx);
        windowing::initialize(cx, hub.clone());
    });
    let mut workspace_out = None;
    let (_, window) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| {
            NebulaWorkspace::new(
                window,
                None,
                None,
                1,
                hub,
                windowing::WorkspaceStartup::Empty,
                windowing::WindowRole::Regular,
                cx,
            )
        });
        workspace.update(cx, |workspace, cx| {
            workspace.tabs_position = nebula_settings::TabsPositionName::Top;
            for index in 0..count {
                let path = directory.path().join(format!("tab-{index}.txt"));
                std::fs::write(&path, "fixture\n").unwrap();
                let view =
                    cx.new(|cx| crate::gpui_shell::code_tab::CodeTabView::new(path, window, cx));
                let subscription = cx.subscribe(&view, |_, _, _, _| {});
                workspace.insert_tab_at(
                    workspace.tabs.len(),
                    WorkspaceTab::Code { view, _subscription: subscription },
                    TabMeta::default(),
                );
            }
            workspace.open_settings(window, cx);
        });
        workspace_out = Some(workspace.clone());
        Root::new(workspace, window, cx)
    });
    window.run_until_parked();
    window.simulate_resize(gpui::size(px(1280.0), px(900.0)));
    workspace_out.as_ref().unwrap().update(window, |workspace, cx| {
        workspace.tabs_position = nebula_settings::TabsPositionName::Top;
        workspace.sync_settings_layout(false);
        cx.notify();
    });
    window.run_until_parked();
    (directory, workspace_out.unwrap(), window.clone())
}

fn click_tab(selector: &'static str, cx: &mut VisualTestContext) {
    let bounds = tab_bounds(selector, cx);
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.run_until_parked();
}

fn tab_bounds(selector: &'static str, cx: &mut VisualTestContext) -> gpui::Bounds<gpui::Pixels> {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    cx.debug_bounds(selector).unwrap_or_else(|| panic!("visible tab bounds: {selector}"))
}

fn numbered_tab_shortcuts() -> [&'static str; 2] {
    if crate::platform::Platform::current() == crate::platform::Platform::MacOS {
        ["cmd-1", "cmd-3"]
    } else {
        ["ctrl-1", "ctrl-3"]
    }
}

#[gpui::test]
fn switching_top_tabs_keeps_settings_reachable_until_explicit_close(cx: &mut TestAppContext) {
    let (_directory, workspace, mut cx) = open_workspace(2, cx);
    let settings_id = workspace.read_with(&cx, |workspace, _| {
        assert_eq!(workspace.tabs_position, nebula_settings::TabsPositionName::Top);
        assert_eq!(workspace.top_tab_count(), 3);
        workspace.settings_surface.as_ref().unwrap().0.entity_id()
    });
    if cx.update(|window, _| {
        crate::platform::window_chrome::layout(window)
            .is_some_and(|layout| layout.native_controls_visible)
    }) {
        let settings = tab_bounds("top-tab-2", &mut cx);
        let title = tab_bounds("workspace-titlebar", &mut cx);
        assert!((settings.center().y - title.center().y).abs() <= px(0.5));
    }
    for (index, selector) in [(0, "top-tab-0"), (1, "top-tab-1"), (0, "top-tab-0")] {
        click_tab(selector, &mut cx);
        workspace.read_with(&cx, |workspace, _| {
            assert!(!workspace.settings_open);
            assert_eq!(workspace.active, index);
            assert_eq!(workspace.top_tab_count(), 3, "switching must retain the settings tab");
        });
        click_tab("top-tab-2", &mut cx);
        workspace.read_with(&cx, |workspace, _| {
            assert!(workspace.settings_open);
            assert_eq!(workspace.top_tab_count(), 3);
            assert_eq!(workspace.settings_surface.as_ref().unwrap().0.entity_id(), settings_id);
        });
    }
    click_tab("top-tab-0", &mut cx);
    let bounds = tab_bounds("top-tab-2", &mut cx);
    cx.simulate_mouse_down(bounds.center(), gpui::MouseButton::Middle, Modifiers::default());
    cx.simulate_mouse_up(bounds.center(), gpui::MouseButton::Middle, Modifiers::default());
    cx.run_until_parked();
    workspace.read_with(&cx, |workspace, _| {
        assert!(!workspace.settings_open);
        assert_eq!(workspace.active, 0);
        assert_eq!(workspace.top_tab_count(), 2);
    });
}

#[gpui::test]
fn keyboard_navigation_can_return_to_background_settings(cx: &mut TestAppContext) {
    let (_directory, workspace, mut cx) = open_workspace(2, cx);
    let [first_tab, third_tab] = numbered_tab_shortcuts();
    for shortcut in [first_tab, "alt-1"] {
        cx.simulate_keystrokes(shortcut);
        assert!(!workspace.read_with(&cx, |workspace, _| workspace.settings_open));
        cx.simulate_keystrokes(third_tab);
        assert!(workspace.read_with(&cx, |workspace, _| workspace.settings_open));
        cx.simulate_keystrokes("ctrl-tab");
        assert!(!workspace.read_with(&cx, |workspace, _| workspace.settings_open));
        cx.simulate_keystrokes("ctrl-shift-tab");
        assert!(workspace.read_with(&cx, |workspace, _| workspace.settings_open));
    }
    cx.simulate_keystrokes("ctrl-shift-w");
    assert_eq!(workspace.read_with(&cx, |workspace, _| workspace.top_tab_count()), 2);
    cx.simulate_keystrokes(third_tab);
    assert!(!workspace.read_with(&cx, |workspace, _| workspace.settings_open));
}

#[gpui::test]
fn settings_survives_overflow_and_layout_changes(cx: &mut TestAppContext) {
    let (_directory, workspace, mut cx) = open_workspace(12, cx);
    cx.simulate_resize(gpui::size(px(760.0), px(900.0)));
    cx.simulate_keystrokes(numbered_tab_shortcuts()[0]);
    for position in
        [nebula_settings::TabsPositionName::Sidebar, nebula_settings::TabsPositionName::Top]
    {
        workspace.update(&mut cx, |workspace, cx| {
            workspace.tabs_position = position;
            workspace.sync_settings_layout(false);
            cx.notify();
        });
        assert_eq!(workspace.read_with(&cx, |workspace, _| workspace.top_tab_count()), 13);
    }
    cx.simulate_keystrokes("ctrl-shift-tab");
    let settings = tab_bounds("top-tab-12", &mut cx);
    assert!(settings.origin.x >= px(0.0) && settings.right() <= px(760.0));
    workspace.read_with(&cx, |workspace, _| assert!(workspace.settings_open));
}

#[gpui::test]
fn instant_slide_animation_keeps_the_side_panel_static(cx: &mut TestAppContext) {
    let (_directory, workspace, mut cx) = open_workspace(1, cx);
    // With the slide animation off, folding the main sidebar for the settings
    // page must not arm the fold animation: the column snaps into place.
    workspace.update(&mut cx, |workspace, cx| {
        workspace.tabs_position = nebula_settings::TabsPositionName::Sidebar;
        workspace.settings_open = true;
        workspace.sidebar_collapsed = false;
        workspace.sync_settings_layout(true);
    });
    workspace.read_with(&cx, |workspace, _| {
        assert!(workspace.sidebar_collapsed, "sidebar mode folds for settings");
        assert!(!workspace.sidebar_fold_armed, "instant: fold animation not armed");
    });
    // The default slide mode keeps arming the fold as before.
    workspace.update(&mut cx, |workspace, _| {
        workspace.sidebar_collapsed = false;
        workspace.sync_settings_layout(false);
    });
    workspace.read_with(&cx, |workspace, _| {
        assert!(workspace.sidebar_fold_armed, "slide: fold animation armed");
    });
    workspace.update(&mut cx, |workspace, _| workspace.sync_settings_layout(true));
    workspace.read_with(&cx, |workspace, _| {
        assert!(workspace.sidebar_collapsed);
        assert!(!workspace.sidebar_fold_armed, "mode switch must clear an unchanged fold");
    });
}

#[gpui::test]
fn closing_last_regular_tab_keeps_settings_until_it_is_closed(cx: &mut TestAppContext) {
    let (_directory, workspace, mut cx) = open_workspace(1, cx);
    click_tab("top-tab-0", &mut cx);
    cx.simulate_keystrokes("ctrl-shift-w");
    workspace.read_with(&cx, |workspace, _| {
        assert!(workspace.tabs.is_empty());
        assert!(workspace.settings_open);
        assert_eq!(workspace.top_tab_count(), 1);
    });
    cx.simulate_keystrokes("ctrl-shift-w");
    assert!(cx.read(|cx| cx.windows().is_empty()));
}

#[gpui::test]
fn density_changes_real_tab_bounds_and_sidebar_drag_pitch(cx: &mut TestAppContext) {
    use crate::gpui_shell::settings_fixture::{SettingsBytesGuard, lock_theme_studio};
    use nebula_settings::{DensityName, TabsPositionName};
    let _lock = lock_theme_studio();
    let _settings = SettingsBytesGuard::capture();

    let (_directory, workspace, mut cx) = open_workspace(8, cx);
    // The initial settings tab can scroll tab 0 outside this eight-tab strip.
    // Establish the layout-test precondition without clicking a clipped row.
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.activate_top_tab(0, window, cx);
            assert!(!workspace.settings_open);
        })
    });
    for position in [TabsPositionName::Sidebar, TabsPositionName::Top] {
        for (density, height, gap, title_height) in [
            (DensityName::Standard, 34.0, 8.0, 48.0),
            (DensityName::Compact, 32.0, 4.0, 48.0),
            (DensityName::Standard, 34.0, 8.0, 48.0),
        ] {
            // Appearance/activation observers reload saved preferences. Use the
            // same source as a real layout selection, rather than relying on
            // the factory default accidentally matching the fixture's mode.
            nebula_settings::persist_keys(&[
                ("tabs_position", position.settings_value().to_owned()),
                ("density", density.settings_value().to_owned()),
            ])
            .unwrap();
            workspace.update(&mut cx, |workspace, cx| {
                workspace.tabs_position = position;
                workspace.density = density;
                workspace.sync_settings_layout(tab_reveal_instant(cx));
                workspace.reveal_active_tab();
                cx.notify();
            });
            cx.run_until_parked();
            let sidebar = position == TabsPositionName::Sidebar;
            let first = tab_bounds(if sidebar { "sidebar-tab-0" } else { "top-tab-0" }, &mut cx);
            let second = tab_bounds(if sidebar { "sidebar-tab-1" } else { "top-tab-1" }, &mut cx);
            let native = cx.update(|window, _| crate::platform::window_chrome::layout(window));
            let title = tab_bounds("workspace-titlebar", &mut cx);
            if !sidebar && native.is_some_and(|layout| layout.native_controls_visible) {
                // A one-pixel bottom border can shift the content center by half a pixel.
                for tab in [first, second] {
                    assert!((tab.center().y - title.center().y).abs() <= px(0.5));
                    assert!((tab.bottom() - title.bottom()).abs() <= px(1.0));
                }
            } else {
                assert_eq!(f32::from(first.size.height), height);
                assert_eq!(f32::from(second.size.height), height);
            }
            if sidebar {
                assert_eq!(f32::from(second.top() - first.top()), height + gap);
            }
            assert_eq!(
                f32::from(title.size.height),
                native.map_or(title_height, |layout| layout.height)
            );

            // Padding belongs to the tab's actual click target in either density.
            let point = gpui::point(second.left() + px(8.0), second.bottom() - px(2.0));
            cx.simulate_click(point, Modifiers::default());
            workspace.read_with(&cx, |workspace, _| assert_eq!(workspace.active, 1));
            if sidebar {
                cx.simulate_mouse_down(point, gpui::MouseButton::Left, Modifiers::default());
                workspace.read_with(&cx, |workspace, _| {
                    assert_eq!(workspace.tab_drag.as_ref().unwrap().pitch, height + gap);
                });
                cx.simulate_mouse_up(point, gpui::MouseButton::Left, Modifiers::default());
            }
        }
    }
}

#[gpui::test]
fn top_terminal_tabs_switch_and_hover_close_the_actual_session(cx: &mut TestAppContext) {
    let (_directory, workspace, mut cx) = open_workspace(0, cx);
    let mut inputs = Vec::new();
    let ids = cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            for name in ["Server Alpha", "Server Beta"] {
                let pane = workspace.new_pane(
                    (80, 24),
                    crate::gpui_shell::terminal::view::TerminalLaunch::Local {
                        cwd: None,
                        shell: Some(nebula_terminal::tty::Shell::new(
                            "pebrel-test-missing-shell-executable".into(),
                            vec![],
                        )),
                        shell_name: None,
                    },
                    None,
                    window,
                    cx,
                );
                inputs.push(pane.view.update(cx, |view, _| view.install_completion_test_session()));
                let id = pane.id;
                workspace.insert_tab_at(
                    workspace.tabs.len(),
                    WorkspaceTab::Terminal {
                        tree: nebula_split::SplitTree::Leaf(id),
                        panes: vec![pane],
                        focused: id,
                        zoomed: false,
                        broadcast: false,
                    },
                    TabMeta { custom_name: Some(name.into()), ..Default::default() },
                );
            }
            workspace.close_top_tab(2, window, cx);
            workspace.activate_top_tab(0, window, cx);
            [
                workspace.tabs[0].focused_view().unwrap().entity_id(),
                workspace.tabs[1].focused_view().unwrap().entity_id(),
            ]
        })
    });
    let second = tab_bounds("top-tab-1", &mut cx);
    cx.simulate_click(
        gpui::point(second.left() + px(8.0), second.center().y),
        Modifiers::default(),
    );
    cx.run_until_parked();
    workspace.read_with(&cx, |workspace, _| {
        assert_eq!(workspace.active, 1);
        assert_eq!(workspace.tabs[1].focused_view().unwrap().entity_id(), ids[1]);
    });
    let first = tab_bounds("top-tab-0", &mut cx);
    cx.simulate_mouse_move(first.center(), None, Modifiers::default());
    let close = tab_bounds("top-close-tab-0", &mut cx);
    assert!(first.contains(&close.center()));
    cx.simulate_click(close.center(), Modifiers::default());
    cx.run_until_parked();
    workspace.read_with(&cx, |workspace, _| {
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active, 0);
        assert_eq!(workspace.tabs[0].focused_view().unwrap().entity_id(), ids[1]);
    });
}

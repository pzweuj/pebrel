//! One workspace-owned sidebar for files, VCS and the active document.
//! Content views own their data; this module owns selection, geometry and gestures.

use super::*;
use crate::display::side_panel::PanelView;
use crate::gpui_shell::file_editor::{DocumentDetails, DocumentSection, TextFileView};
use crate::gpui_shell::widgets::toolbar_button;
use crate::i18n::Message;

#[cfg(all(test, feature = "gpui-test-support"))]
#[path = "details_panel_tests.rs"]
mod tests;

const DEFAULT_WIDTH: f32 = 320.0;
const MIN_WIDTH: f32 = 240.0;
const MAX_WIDTH: f32 = 560.0;
const RESIZE_HIT_WIDTH: f32 = 8.0;
const HEADER_CONTROL_SIZE: f32 = 32.0;

pub(super) struct DetailsPanelState {
    pub(super) section: Option<DocumentSection>,
    width: f32,
    closing_width: Option<f32>,
    resize: Option<super::sidebar_resize::ResizeDrag>,
    document: Option<(Entity<TextFileView>, Entity<DocumentDetails>)>,
    tab: Option<u8>,
    previous_tab: Option<u8>,
    transition: u64,
}

impl Default for DetailsPanelState {
    fn default() -> Self {
        Self {
            section: Some(DocumentSection::Outline),
            width: DEFAULT_WIDTH,
            closing_width: None,
            resize: None,
            document: None,
            tab: None,
            previous_tab: None,
            transition: 0,
        }
    }
}

fn panel_width(preferred: f32, available: f32) -> f32 {
    // Preserve a useful document/terminal area in narrow windows. Do not change
    // the preferred width when the window temporarily gets smaller.
    let maximum = (available * 0.46).clamp(160.0, MAX_WIDTH);
    preferred.clamp(MIN_WIDTH.min(maximum), maximum)
}

impl NebulaWorkspace {
    pub(super) fn toggle_side_panel(
        &mut self,
        view: crate::display::side_panel::PanelView,
        cx: &mut Context<Self>,
    ) {
        self.side_panel_anim_armed = !tab_reveal_instant(cx);
        self.side_panel.toggle(view);
        self.file_tree_menu = None;
        if !self.side_panel.open {
            cx.notify();
            return;
        }

        self.sync_side_panel_to_active(true, cx);

        // The shared model builds snapshots on a worker and exposes a cheap,
        // throttled `sync`. GPUI polls only while the drawer is open; it does
        // not move filesystem business logic into the render function.
        if !self.side_panel_polling {
            self.side_panel_polling = true;
            let executor = cx.background_executor().clone();
            cx.spawn(async move |this, cx| {
                loop {
                    executor.timer(Duration::from_millis(100)).await;
                    let keep_polling = this
                        .update(cx, |workspace, cx| {
                            if !workspace.side_panel.open {
                                workspace.side_panel_polling = false;
                                return false;
                            }
                            if workspace.sync_side_panel_to_active(false, cx) {
                                cx.notify();
                            }
                            true
                        })
                        .unwrap_or(false);
                    if !keep_polling {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn active_details_document(&self, _cx: &App) -> Option<Entity<TextFileView>> {
        if self.settings_open {
            return None;
        }
        match self.tabs.get(self.active) {
            Some(WorkspaceTab::Document { view, .. }) => Some(view.clone()),
            _ => None,
        }
    }

    pub(super) fn active_document_section(&self, cx: &App) -> Option<DocumentSection> {
        let document = self.active_details_document(cx)?;
        self.details_panel.section.filter(|section| {
            *section != DocumentSection::Outline || document.read(cx).has_outline()
        })
    }

    pub(super) fn sync_document_details(&mut self, cx: &mut Context<Self>) {
        let document = self.active_details_document(cx);
        if document.as_ref() != self.details_panel.document.as_ref().map(|(file, _)| file) {
            self.details_panel.resize = None;
            self.details_panel.document = document.clone().map(|file| {
                let section = self.details_panel.section.unwrap_or(DocumentSection::Outline);
                let panel = cx.new(|cx| DocumentDetails::new(file.clone(), section, cx));
                (file, panel)
            });
        }
        if let Some((file, panel)) = &self.details_panel.document {
            let open = self.side_panel.open;
            file.update(cx, |view, cx| view.host_details(open, cx));
            if let Some(section) = self.active_document_section(cx) {
                panel.update(cx, |view, cx| view.set_section(section, cx));
            }
        }
        if !self.side_panel.open || self.reader_focus_active(cx) {
            self.details_panel.resize = None;
        }
        if self.side_panel.open || self.side_panel_anim_armed {
            let active = self.details_tab(cx);
            if self.details_panel.tab != Some(active) {
                self.details_panel.previous_tab = self.details_panel.tab;
                self.details_panel.tab = Some(active);
                self.details_panel.transition = self.details_panel.transition.wrapping_add(1);
            }
        }
    }

    pub(super) fn files_panel_on_left(&self, cx: &App) -> bool {
        cx.try_global::<crate::gpui_shell::config::Settings>().is_some_and(|settings| {
            settings.files_panel_side == nebula_settings::FilesPanelSide::Left
        })
    }

    fn details_panel_display_width(&self, window: &Window) -> f32 {
        if !self.side_panel.open && self.details_panel.closing_width.is_some() {
            self.details_panel.closing_width.unwrap()
        } else if self.details_panel.resize.is_some() {
            self.details_panel.width
        } else {
            panel_width(self.details_panel.width, f32::from(window.viewport_size().width))
        }
    }

    /// 左侧文件/详情抽屉的宽度与展开方向；停靠右侧、专注模式或已关闭且无收起
    /// 动画时返回 `None`，供顶部标签栏与拖拽滚动同步让位。
    pub(super) fn left_details_panel_layout(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(f32, bool)> {
        if !self.files_panel_on_left(cx) || self.reader_focus_active(cx) {
            return None;
        }
        let open = self.side_panel.open;
        if !open && (tab_reveal_instant(cx) || !self.side_panel_anim_armed) {
            return None;
        }
        Some((self.details_panel_display_width(window), open))
    }

    /// 顶部标签且展开栏停靠左侧（无 macOS 原生交通灯）时，把抽屉顶栏直接嵌入
    /// 标题栏左侧让位区，避免展开栏上方出现 48px 空白标题带叠加 48px 抽屉顶栏。
    pub(super) fn details_header_in_title_bar(&self, window: &Window, cx: &App) -> bool {
        self.tabs_position == nebula_settings::TabsPositionName::Top
            && !self.settings_open
            && crate::platform::window_chrome::layout(window).is_none()
            && self.left_details_panel_layout(window, cx).is_some()
    }

    fn left_sidebar_occupied_width(&self) -> f32 {
        if self.tabs_position == nebula_settings::TabsPositionName::Sidebar
            && !self.sidebar_collapsed
        {
            self.sidebar_width
        } else {
            0.0
        }
    }

    fn toggle_files_panel_side(&mut self, cx: &mut Context<Self>) {
        let next = if self.files_panel_on_left(cx) {
            nebula_settings::FilesPanelSide::Right
        } else {
            nebula_settings::FilesPanelSide::Left
        };
        if nebula_settings::persist_keys(&[("files_panel_side", next.settings_value().to_owned())])
            .is_err()
        {
            return;
        }
        self.apply_runtime_settings(cx);
    }

    pub(super) fn toggle_document_details(&mut self, cx: &mut Context<Self>) {
        if self.side_panel.open {
            self.toggle_side_panel(self.side_panel.view, cx);
        } else {
            self.select_document_section(DocumentSection::Outline, cx);
        }
    }

    pub(super) fn render_right_sidebar_button(
        &self,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let visible = self.side_panel.open && !self.reader_focus_active(cx);
        let icon =
            if self.files_panel_on_left(cx) { IconName::PanelLeft } else { IconName::PanelRight };
        toolbar_button("toggle-right-sidebar", icon, disabled)
            .debug_selector(|| "toggle-right-sidebar".to_owned())
            .selected(visible)
            .when(visible, |button| button.bg(cx.theme().secondary))
            .tooltip(crate::gpui_shell::config::ui_language(cx).text(Message::EditorRightSidebar))
            .on_click(cx.listener(|this, _, _, cx| {
                if this.reader_focus_active(cx) {
                    this.clear_reader_focus(cx);
                    if !this.side_panel.open {
                        this.toggle_side_panel(this.side_panel.view, cx);
                    }
                } else {
                    this.toggle_side_panel(this.side_panel.view, cx);
                }
            }))
    }

    fn select_document_section(&mut self, section: DocumentSection, cx: &mut Context<Self>) {
        let Some(file) = self.active_details_document(cx) else { return };
        let section = if section == DocumentSection::Outline && !file.read(cx).has_outline() {
            DocumentSection::Info
        } else {
            section
        };
        if !self.side_panel.open {
            self.toggle_side_panel(self.side_panel.view, cx);
        }
        self.details_panel.section = Some(section);
        self.file_tree_menu = None;
        cx.notify();
    }

    fn details_tab(&self, cx: &App) -> u8 {
        match self.active_document_section(cx) {
            Some(DocumentSection::Info) => 0,
            Some(DocumentSection::Outline) => 1,
            None if self.side_panel.view == PanelView::Files => 2,
            None => 3,
        }
    }

    pub(super) fn render_details_header(
        &self,
        width: f32,
        in_title_bar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let language = crate::gpui_shell::config::ui_language(cx);
        let document = self.active_details_document(cx);
        let vcs_name = match self.side_panel.vcs() {
            Some(crate::display::side_panel::VcsKind::Svn)
            | Some(crate::display::side_panel::VcsKind::SvnRepository) => "SVN",
            _ => "Git",
        };
        let mut tabs = Vec::new();
        if let Some(document) = &document {
            tabs.push((0, "details-info", language.text(Message::EditorInfo), IconName::Info));
            if document.read(cx).has_outline() {
                tabs.push((
                    1,
                    "details-outline",
                    language.text(Message::EditorOutline),
                    IconName::Menu,
                ));
            }
        }
        tabs.push((
            2,
            "side-panel-files",
            language.text(Message::CommonFiles),
            IconName::FolderClosed,
        ));
        tabs.push((3, "side-panel-git", vcs_name, IconName::Github));
        let controls = tabs.len() + 1;
        let label_limit = (width - 16.0 - controls as f32 * (HEADER_CONTROL_SIZE + 2.0)).max(0.0);
        let selected = self.details_tab(cx);
        let previous = self.details_panel.previous_tab;
        let serial = self.details_panel.transition;
        let background = cx.theme().secondary;
        let files_on_left = self.files_panel_on_left(cx);
        let swap_tip = language.text(Message::SettingsFilesDrawerSide);
        let mut tab_group = h_flex().gap(px(2.0)).items_center();
        if in_title_bar {
            tab_group = tab_group.occlude();
        }
        for (index, id, label, icon) in tabs {
            let measured = window.text_system().shape_line(
                label.into(),
                px(13.0),
                &[window.text_style().to_run(label.len())],
                None,
            );
            let extra = (f32::from(measured.width) + 10.0).min(label_limit);
            let from = if previous == Some(index) { 1.0 } else { 0.0 };
            let to = if selected == index { 1.0 } else { 0.0 };
            let tooltip = label;
            let label = div()
                .flex_shrink_0()
                .overflow_hidden()
                .text_size(px(13.0))
                .child(div().w(px(extra)).pr(px(8.0)).truncate().child(label))
                .with_animation(
                    (id, serial),
                    Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
                    move |label, t| {
                        label
                            .w(px(extra * (from + (to - from) * t)))
                            .opacity(from + (to - from) * t)
                    },
                );
            let button = Button::new(id)
                .ghost()
                .h(px(HEADER_CONTROL_SIZE))
                .min_w(px(HEADER_CONTROL_SIZE))
                .px_0()
                .flex_shrink_0()
                .overflow_hidden()
                .tooltip(tooltip)
                .selected(selected == index)
                .debug_selector(move || id.to_owned())
                .child(
                    h_flex()
                        .gap_0()
                        .child(
                            div()
                                .w(px(HEADER_CONTROL_SIZE))
                                .h(px(HEADER_CONTROL_SIZE))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(icon).size(px(16.0))),
                        )
                        .child(label),
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    if view.side_panel.open && view.details_tab(cx) == index {
                        view.toggle_side_panel(view.side_panel.view, cx);
                        return;
                    }
                    match index {
                        0 => view.select_document_section(DocumentSection::Info, cx),
                        1 => view.select_document_section(DocumentSection::Outline, cx),
                        _ => {
                            view.details_panel.section = None;
                            view.select_side_panel_view(
                                if index == 2 { PanelView::Files } else { PanelView::Git },
                                cx,
                            );
                        },
                    }
                }))
                .with_animation(
                    (id, serial),
                    Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
                    move |button, t| {
                        let progress = from + (to - from) * t;
                        button
                            .w(px(HEADER_CONTROL_SIZE + extra * progress))
                            .bg(background.opacity(progress))
                    },
                );
            tab_group = tab_group.child(button);
        }
        let swap_button = div().flex_shrink_0().when(in_title_bar, |slot| slot.occlude()).child(
            toolbar_button(
                "files-panel-side",
                if files_on_left { IconName::PanelRight } else { IconName::PanelLeft },
                false,
            )
            .debug_selector(|| "files-panel-side".to_owned())
            .tooltip(swap_tip)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_files_panel_side(cx))),
        );
        h_flex()
            .id("workspace-details-header")
            .debug_selector(|| "workspace-details-header".to_owned())
            .when(in_title_bar, |header| header.h_full())
            .when(!in_title_bar, |header| header.h(px(40.0)))
            .w_full()
            .flex_shrink_0()
            .px(px(8.0))
            .gap(px(2.0))
            .items_center()
            .child(tab_group)
            .child(div().flex_1())
            .child(swap_button)
            .into_any_element()
    }

    pub(super) fn render_side_panel_slot(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let instant = tab_reveal_instant(cx);
        if instant {
            // 切换偏好时面板可能已经关闭，旧动画标志不得保留布局占位。
            self.side_panel_anim_armed = false;
        }
        if !self.side_panel_anim_armed && !self.side_panel.open {
            return div().into_any_element();
        }
        let active = self.details_tab(cx);
        if self.details_panel.tab != Some(active) {
            self.details_panel.previous_tab = self.details_panel.tab;
            self.details_panel.tab = Some(active);
            self.details_panel.transition = self.details_panel.transition.wrapping_add(1);
        }
        let open = self.side_panel.open;
        if open {
            self.details_panel.closing_width = None;
        }
        let width = self.details_panel_display_width(window);
        let section = self.active_document_section(cx);
        let remote = open && section.is_none() && self.route_remote_browser(window, cx);
        let panel = if section.is_some() {
            self.details_panel.document.as_ref().unwrap().1.clone().into_any_element()
        } else {
            match self.side_panel.view {
                PanelView::Files if remote => self.render_remote_files(window, cx),
                PanelView::Files => self.render_file_tree(cx),
                PanelView::Git => self.render_git_tree(window, cx),
            }
        };
        let header_in_title_bar = self.details_header_in_title_bar(window, cx);
        let band = v_flex()
            .relative()
            .w(px(width))
            .h_full()
            .pb(px(if remote && matches!(self.side_panel.view, PanelView::Files) {
                0.0
            } else {
                crate::gpui_shell::theme::PaneCardStyle::current(cx).margin.bottom
            }))
            .when(!header_in_title_bar, |band| {
                band.child(self.render_details_header(width, false, window, cx))
            })
            .child(div().flex_1().min_h_0().w_full().child(panel).with_animation(
                ("details-content", self.details_panel.transition),
                Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()),
                |content, t| content.opacity(t),
            ));
        // Instant: both 240 ms translations are dropped and the band rests at
        // its final width, so the panel appears in place instead of sliding.
        let from_left = self.files_panel_on_left(cx);
        let band: gpui::AnyElement = if instant {
            band.into_any_element()
        } else {
            band.with_animation(
                ("side-panel-push", (open as usize) + if from_left { 2 } else { 0 }),
                Animation::new(Duration::from_millis(240)).with_easing(ease_out_quint()),
                move |band, t| {
                    let travel = width * if open { 1.0 - t } else { t };
                    band.left(px(if from_left { -travel } else { travel }))
                },
            )
            .into_any_element()
        };
        let titlebar_background = self.titlebar_background.clone();
        let slot = div()
            .id("workspace-details-slot")
            .debug_selector(|| "workspace-details-slot".to_owned())
            .relative()
            .h_full()
            .flex_shrink_0()
            .when(from_left, |slot| {
                slot.child(
                    gpui::canvas(
                        move |bounds, _, _| titlebar_background.record_left_drawer(bounds),
                        |_, _, _, _| (),
                    )
                    .absolute()
                    .inset_0(),
                )
            })
            .child(div().size_full().overflow_hidden().bg(cx.theme().background).child(band));
        if instant {
            slot.w(px(width)).into_any_element()
        } else {
            slot.with_animation(
                ("side-panel-slide", open as usize),
                Animation::new(Duration::from_millis(240)).with_easing(ease_out_quint()),
                move |slot, t| slot.w(px(width * if open { t } else { 1.0 - t })),
            )
            .into_any_element()
        }
    }

    pub(super) fn render_details_panel_resize_handle(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.side_panel.open || self.reader_focus_active(cx) || self.settings_open {
            return None;
        }
        let width = self.details_panel_display_width(window);
        let on_left = self.files_panel_on_left(cx);
        let sidebar = if on_left { self.left_sidebar_occupied_width() } else { 0.0 };
        Some(
            div()
                .id("workspace-details-resize")
                .debug_selector(|| "workspace-details-resize".to_owned())
                .absolute()
                .top_0()
                .bottom_0()
                .when(on_left, |handle| handle.left(px(sidebar + width - RESIZE_HIT_WIDTH * 0.5)))
                .when(!on_left, |handle| handle.right(px(width - RESIZE_HIT_WIDTH * 0.5)))
                .w(px(RESIZE_HIT_WIDTH))
                .occlude()
                .cursor_col_resize()
                .group("details-divider")
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(RESIZE_HIT_WIDTH * 0.5 - 1.0))
                        .w(px(1.0))
                        .bg(cx.theme().border)
                        .group_hover("details-divider", |line| line.bg(cx.theme().ring))
                        .when(self.details_panel.resize.is_some(), |line| line.bg(cx.theme().ring)),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        gpui_component::GlobalState::suppress_text_selection(cx);
                        this.details_panel.resize = Some(super::sidebar_resize::ResizeDrag::new(
                            f32::from(event.position.x),
                            width,
                        ));
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn details_panel_resizing(&self) -> bool {
        self.details_panel.resize.is_some()
    }

    pub(super) fn update_details_panel_resize(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let on_left = self.files_panel_on_left(cx);
        let Some(drag) = &mut self.details_panel.resize else { return };
        if !window.is_window_active() {
            self.cancel_details_panel_resize(cx);
            return;
        }
        // Non-client moves can omit button state while capture is still active.
        // Only MouseUp commits the drag; Escape or window deactivation cancels it.
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let maximum = panel_width(MAX_WIDTH, f32::from(window.viewport_size().width));
        let minimum = MIN_WIDTH.min(maximum);
        let x = f32::from(event.position.x);
        // The handle is the inner edge. Dragging away from the session grows
        // the drawer: leftward when it is docked on the right, rightward on the left.
        let raw = if on_left {
            drag.start_width + x - drag.start_x
        } else {
            drag.start_width + drag.start_x - x
        };
        self.details_panel.width = drag.width(raw, minimum, maximum);
        cx.notify();
    }

    pub(super) fn finish_details_panel_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.details_panel.resize.take() else { return false };
        if drag.close {
            self.details_panel.closing_width = Some(self.details_panel.width);
        }
        self.details_panel.width = if drag.close { drag.start_width } else { drag.open_width };
        if drag.close {
            self.toggle_side_panel(self.side_panel.view, cx);
        }
        cx.notify();
        true
    }

    pub(super) fn cancel_details_panel_resize(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.details_panel.resize.take() else { return false };
        self.details_panel.width = drag.start_width;
        cx.notify();
        true
    }

    pub(super) fn render_details_panel_resize_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let on_left = self.files_panel_on_left(cx);
        let hint_offset = if on_left { self.left_sidebar_occupied_width() } else { 0.0 }
            + self.details_panel.width;
        self.details_panel.resize.map(|drag| {
            div()
                .id("workspace-details-resize-overlay")
                .absolute()
                .inset_0()
                .occlude()
                .cursor_col_resize()
                .on_mouse_move(cx.listener(|this, event, window, cx| {
                    this.update_details_panel_resize(event, window, cx);
                    cx.stop_propagation();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.finish_details_panel_resize(cx);
                        cx.stop_propagation();
                    }),
                )
                .when(drag.close, |overlay| {
                    overlay.child(super::sidebar_resize::collapse_hint(on_left, hint_offset, cx))
                })
                .into_any_element()
        })
    }
}

use super::*;

impl SettingsPane {
    pub(super) fn about_action_row(
        id: &'static str,
        icon: IconName,
        title: &'static str,
        url: String,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        Self::about_row(id, icon, title, IconName::ExternalLink, cx)
            .on_click(move |_, _, cx| cx.open_url(&url))
            .into_any_element()
    }

    /// 与外链行同一条骨架，尾标是「进入页面」而不是「离开应用」。
    pub(super) fn about_page_row(
        id: &'static str,
        icon: IconName,
        title: &'static str,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        Self::about_row(id, icon, title, IconName::ChevronRight, cx)
            .on_click(on_click)
            .into_any_element()
    }

    fn about_row(
        id: &'static str,
        icon: IconName,
        title: &'static str,
        trailing: IconName,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().list_hover;
        h_flex()
            .id(id)
            .debug_selector(move || id.to_owned())
            .w_full()
            .h(px(48.0))
            .flex_shrink_0()
            .px_3()
            .gap_3()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .child(Icon::new(icon).small().text_color(muted))
            .child(div().flex_1().min_w_0().child(title))
            .child(Icon::new(trailing).xsmall().text_color(muted))
    }

    pub(super) fn about_value_row(
        label: &'static str,
        value: impl IntoElement,
        cx: &Context<Self>,
    ) -> gpui::Div {
        h_flex()
            .w_full()
            .h(px(48.0))
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap_4()
            .child(div().flex_1().min_w_0().text_color(cx.theme().foreground).child(label))
            .child(value)
    }

    pub(super) fn section_home(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let language = crate::gpui_shell::config::ui_language(cx);
        let muted = cx.theme().muted_foreground;
        let ink = cx.theme().foreground;
        let base_px = self.font_size_px(cx);
        let identity =
            h_flex()
                .w_full()
                .items_center()
                .gap(px(24.0))
                .pb(px(64.0))
                .when_some(
                    crate::app_icon::preview(
                        crate::app_icon::selected(),
                        (96.0 * window.scale_factor()).round() as u32,
                    ),
                    |row, logo| row.child(img(logo).size(px(96.0)).flex_shrink_0()),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(7.0))
                        .child(
                            div()
                                .text_size(px(base_px * 2.15))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(ink)
                                .child(crate::brand::NAME),
                        )
                        .child(div().text_color(muted).child(language.text(
                            crate::i18n::Message::SettingsAboutGpuAcceleratedTerminalWindows,
                        )))
                        .child(
                            div()
                                .mt(px(12.0))
                                .text_size(px(base_px * 0.88))
                                .text_color(muted)
                                .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                        ),
                );
        let section_title = |title: &'static str| {
            div()
                .mb_4()
                .text_size(px(base_px * 1.15))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title)
        };
        let actions = v_flex()
            .flex_1()
            .min_w(px(280.0))
            .child(section_title(
                language.text(crate::i18n::Message::SettingsAboutProjectAndSupport),
            ))
            .child(Self::about_action_row(
                "about-report-issue",
                IconName::TriangleAlert,
                language.text(crate::i18n::Message::SettingsAboutReportAnIssue),
                issue_url(),
                cx,
            ))
            .child(Self::about_action_row(
                "about-github",
                IconName::Github,
                language.text(crate::i18n::Message::SettingsAboutGithubRepository),
                REPOSITORY_URL.to_owned(),
                cx,
            ))
            .child(Self::about_page_row(
                "about-sponsors",
                IconName::Heart,
                language.text(crate::i18n::Message::SettingsAboutSponsors),
                cx.listener(|this, _, _, cx| this.open_sponsor_page(cx)),
                cx,
            ));

        v_flex().w_full().child(identity).child(actions)
    }
}

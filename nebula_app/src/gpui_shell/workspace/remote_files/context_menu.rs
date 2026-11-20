//! Context menu and file mutation dialogs (delete, rename, create folder) for SFTP rows.

use gpui::{ClipboardItem, MouseDownEvent, Pixels, Point};
use gpui_component::menu::PopupMenuItem;

use super::*;
use crate::i18n::Message;

impl NebulaWorkspace {
    pub(super) fn open_remote_row_context_menu(
        &mut self,
        row: SftpEntry,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if row.is_parent {
            self.open_remote_empty_context_menu(event.position, window, cx);
            return;
        }
        if !self.remote_browser.selection.contains(&row.path) {
            self.remote_browser.selection.select_only(&row.path);
        }
        if let Some(focus) = &self.remote_browser.focus_handle {
            focus.focus(window, cx);
        }
        let selected = self.selected_remote_entries();
        let busy = self.remote_transfer_working() || self.remote_browser.preflighting;
        let ready = self.current_remote_transfer_target().is_some();
        let has_clipboard = self.remote_browser.clipboard.is_some();
        let single = selected.len() <= 1;
        let primary = if single { row } else { selected[0].clone() };
        let workspace = cx.entity().downgrade();
        let language = crate::gpui_shell::config::ui_language(cx);

        let menu = PopupMenu::build(window, cx, move |mut menu, _window, _cx| {
            menu = menu.external_link_icon(false);
            if single {
                if primary.kind == SftpEntryKind::Directory {
                    let open_ws = workspace.clone();
                    let open_entry = primary.clone();
                    menu = menu.item(
                        PopupMenuItem::new(language.text(Message::CommonOpen))
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, window, cx| {
                                if let Some(workspace) = open_ws.upgrade() {
                                    workspace.update(cx, |this, cx| {
                                        this.remote_activate(open_entry.clone(), true, window, cx);
                                    });
                                }
                            }),
                    );
                } else {
                    let ext_ws = workspace.clone();
                    let ext_entry = primary.clone();
                    menu = menu.item(
                        PopupMenuItem::new(language.text(Message::RemoteOpenWithDefault))
                            .icon(IconName::ExternalLink)
                            .disabled(!ready)
                            .on_click(move |_, window, cx| {
                                if let Some(workspace) = ext_ws.upgrade() {
                                    workspace.update(cx, |this, cx| {
                                        this.open_remote_file_with_default_app(
                                            ext_entry.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    );
                    let tab_ws = workspace.clone();
                    let tab_path = primary.path.clone();
                    menu = menu.item(
                        PopupMenuItem::new(language.text(Message::RemoteOpenInTab))
                            .icon(IconName::File)
                            .disabled(!ready)
                            .on_click(move |_, window, cx| {
                                if let Some(workspace) = tab_ws.upgrade() {
                                    workspace.update(cx, |this, cx| {
                                        this.open_remote_document(
                                            this.remote_browser.destination.clone(),
                                            tab_path.clone(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }),
                    );
                }
                menu = menu.separator();
            }

            let dl_ws = workspace.clone();
            let copy_ws = workspace.clone();
            let paste_ws = workspace.clone();
            menu = menu
                .item(
                    PopupMenuItem::new(language.text(Message::RemoteDownload))
                        .icon(IconName::ArrowDown)
                        .disabled(!ready || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = dl_ws.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.remote_pick_download_directory(window, cx);
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new(language.text(Message::RemoteCopyItem))
                        .icon(IconName::Copy)
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = copy_ws.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.remote_copy_selected(window, cx);
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new(language.text(Message::RemotePasteHere))
                        .icon(IconName::Inbox)
                        .disabled(!ready || !has_clipboard || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = paste_ws.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.remote_paste(window, cx);
                                });
                            }
                        }),
                );

            if single {
                let copy_path = primary.path.clone();
                menu = menu.item(
                    PopupMenuItem::new(language.text(Message::RemoteCopyPath))
                        .icon(IconName::Copy)
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()));
                            crate::gpui_shell::toast::toast(
                                window,
                                cx,
                                crate::display::ToastKind::Info,
                                language.text(Message::RemoteCopied),
                            );
                        }),
                );
            }

            menu = menu.separator();

            if single {
                let rename_ws = workspace.clone();
                let rename_entry = primary.clone();
                menu = menu.item(
                    PopupMenuItem::new(language.text(Message::CommonRename))
                        .disabled(!ready || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = rename_ws.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.request_rename_remote_entry(
                                        rename_entry.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                );
            }

            let mkdir_ws = workspace.clone();
            let delete_ws = workspace.clone();
            let upload_file_ws = workspace.clone();
            let upload_dir_ws = workspace.clone();
            let refresh_ws = workspace.clone();

            menu.item(
                PopupMenuItem::new(language.text(Message::CommonNewFolder))
                    .icon(IconName::Folder)
                    .disabled(!ready || busy)
                    .on_click(move |_, window, cx| {
                        if let Some(workspace) = mkdir_ws.upgrade() {
                            workspace.update(cx, |this, cx| {
                                this.request_new_remote_folder(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(language.text(Message::CommonDelete))
                    .icon(IconName::Delete)
                    .disabled(!ready || busy)
                    .on_click(move |_, window, cx| {
                        if let Some(workspace) = delete_ws.upgrade() {
                            workspace.update(cx, |this, cx| {
                                this.request_delete_remote_selection(window, cx);
                            });
                        }
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(language.text(Message::RemoteUploadFiles))
                    .icon(IconName::ArrowUp)
                    .disabled(!ready || busy)
                    .on_click(move |_, window, cx| {
                        if let Some(workspace) = upload_file_ws.upgrade() {
                            workspace.update(cx, |this, cx| {
                                this.remote_pick_upload_files(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(language.text(Message::RemoteUploadFolder))
                    .icon(IconName::ArrowUp)
                    .disabled(!ready || busy)
                    .on_click(move |_, window, cx| {
                        if let Some(workspace) = upload_dir_ws.upgrade() {
                            workspace.update(cx, |this, cx| {
                                this.remote_pick_upload_directory(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(language.text(Message::RemoteReread))
                    .icon(IconName::Redo2)
                    .on_click(move |_, _, cx| {
                        if let Some(workspace) = refresh_ws.upgrade() {
                            workspace.update(cx, |this, cx| this.remote_refresh(cx));
                        }
                    }),
            )
        });

        self.show_file_tree_context_menu(menu, event.position, window, cx);
    }

    pub(super) fn open_remote_empty_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let busy = self.remote_transfer_working() || self.remote_browser.preflighting;
        let ready = self.current_remote_transfer_target().is_some();
        let has_clipboard = self.remote_browser.clipboard.is_some();
        let current_path = self.remote_browser.path.clone();
        let workspace = cx.entity().downgrade();
        let language = crate::gpui_shell::config::ui_language(cx);

        let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
            let upload_file_ws = workspace.clone();
            let upload_dir_ws = workspace.clone();
            let mkdir_ws = workspace.clone();
            let paste_ws = workspace.clone();
            let refresh_ws = workspace.clone();
            let has_path = !current_path.is_empty();

            menu.external_link_icon(false)
                .item(
                    PopupMenuItem::new(language.text(Message::RemoteUploadFiles))
                        .icon(IconName::ArrowUp)
                        .disabled(!ready || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = upload_file_ws.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.remote_pick_upload_files(window, cx);
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new(language.text(Message::RemoteUploadFolder))
                        .icon(IconName::ArrowUp)
                        .disabled(!ready || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = upload_dir_ws.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.remote_pick_upload_directory(window, cx);
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new(language.text(Message::CommonNewFolder))
                        .icon(IconName::Folder)
                        .disabled(!ready || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = mkdir_ws.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.request_new_remote_folder(window, cx);
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new(language.text(Message::RemotePasteHere))
                        .icon(IconName::Inbox)
                        .disabled(!ready || !has_clipboard || busy)
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = paste_ws.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.remote_paste(window, cx);
                                });
                            }
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new(language.text(Message::RemoteCopyPath))
                        .icon(IconName::Copy)
                        .disabled(!has_path)
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(current_path.clone()));
                            crate::gpui_shell::toast::toast(
                                window,
                                cx,
                                crate::display::ToastKind::Info,
                                language.text(Message::RemoteCopied),
                            );
                        }),
                )
                .item(
                    PopupMenuItem::new(language.text(Message::RemoteReread))
                        .icon(IconName::Redo2)
                        .on_click(move |_, _, cx| {
                            if let Some(workspace) = refresh_ws.upgrade() {
                                workspace.update(cx, |this, cx| this.remote_refresh(cx));
                            }
                        }),
                )
        });

        self.show_file_tree_context_menu(menu, position, window, cx);
    }

    pub(super) fn request_delete_remote_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.remote_transfer_working() || self.remote_browser.preflighting {
            return;
        }
        let Some(target) = self.current_remote_transfer_target() else { return };
        let entries = self.selected_remote_entries();
        if entries.is_empty() {
            return;
        }
        let language = crate::gpui_shell::config::ui_language(cx);
        let (title, body): (SharedString, SharedString) = if entries.len() == 1 {
            let entry = &entries[0];
            let title = language.format(
                Message::ConfirmationRemoteDeleteTitle,
                &[("name", &crate::display::truncate_tab_label(&entry.name, 28))],
            );
            let body = if entry.kind == SftpEntryKind::Directory {
                language.text(Message::ConfirmationRemoteDeleteDirectoryBody).to_owned()
            } else {
                language.text(Message::ConfirmationRemoteDeleteFileBody).to_owned()
            };
            (title.into(), body.into())
        } else {
            let title = language.format(
                Message::RemoteDeleteSelectedTitle,
                &[("count", &entries.len().to_string())],
            );
            let body = language.text(Message::RemoteDeleteSelectedBody).to_owned();
            (title.into(), body.into())
        };

        let workspace = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, _cx| {
            let workspace = workspace.clone();
            let target = target.clone();
            let entries = entries.clone();
            confirm_dialog(
                dialog,
                window,
                title.clone(),
                body.clone(),
                language.text(Message::CommonDelete),
                language.text(Message::CommonCancel),
                ButtonVariant::Danger,
            )
            .on_ok(move |_, _, cx| {
                if let Some(workspace) = workspace.upgrade() {
                    let _ = workspace.update(cx, |this, cx| {
                        this.execute_remote_delete(entries.clone(), &target, cx);
                    });
                }
                true
            })
        });
    }

    fn execute_remote_delete(
        &mut self,
        entries: Vec<SftpEntry>,
        target: &RemoteTransferTarget,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.remote_transfer_target_matches(target) {
            self.remote_browser.error =
                Some(workspace_ui_language().text(Message::TransferTargetChanged).to_owned());
            cx.notify();
            return;
        }
        match self.remote_transfer_controller(target, cx) {
            Ok(controller) => {
                self.remote_browser.error = None;
                controller.delete_entries(entries);
                cx.notify();
            },
            Err(message) => {
                self.remote_browser.error = Some(message);
                cx.notify();
            },
        }
    }

    pub(super) fn request_rename_remote_entry(
        &mut self,
        entry: SftpEntry,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if entry.is_parent || self.remote_transfer_working() || self.remote_browser.preflighting {
            return;
        }
        let Some(target) = self.current_remote_transfer_target() else { return };
        let language = crate::gpui_shell::config::ui_language(cx);
        let original = entry.name.clone();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .placeholder(language.text(Message::RemoteRenamePlaceholder));
            state.set_value(original.clone(), window, cx);
            state.set_selected_range(0..original.len(), cx);
            state
        });
        let focus_input = input.clone();
        let workspace = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, _cx| {
            let submit_input = input.clone();
            let workspace = workspace.clone();
            let target = target.clone();
            let entry = entry.clone();
            confirm_dialog(
                dialog,
                window,
                language.text(Message::RemoteRenameTitle),
                entry.path.clone(),
                language.text(Message::CommonRename),
                language.text(Message::CommonCancel),
                ButtonVariant::Primary,
            )
            .child(div().w_full().pt_1().child(Input::new(&input).w_full()))
            .on_ok(move |_, window, cx| {
                let next_name = submit_input.read(cx).value().trim().to_owned();
                if next_name == entry.name {
                    return true;
                }
                if let Err(message) = crate::ssh_sftp::validate_name(&next_name) {
                    crate::gpui_shell::toast::toast(
                        window,
                        cx,
                        crate::display::ToastKind::Warning,
                        message,
                    );
                    return false;
                }
                if let Some(workspace) = workspace.upgrade() {
                    let _ = workspace.update(cx, |this, cx| {
                        this.execute_remote_rename(entry.clone(), &next_name, &target, cx);
                    });
                }
                true
            })
        });
        focus_input.update(cx, |state, cx| state.focus(window, cx));
    }

    fn execute_remote_rename(
        &mut self,
        entry: SftpEntry,
        new_name: &str,
        target: &RemoteTransferTarget,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.remote_transfer_target_matches(target) {
            self.remote_browser.error =
                Some(workspace_ui_language().text(Message::TransferTargetChanged).to_owned());
            cx.notify();
            return;
        }
        match self.remote_transfer_controller(target, cx) {
            Ok(controller) => {
                self.remote_browser.error = None;
                if let Err(message) = controller.rename(entry, new_name) {
                    self.remote_browser.error = Some(message);
                }
                cx.notify();
            },
            Err(message) => {
                self.remote_browser.error = Some(message);
                cx.notify();
            },
        }
    }

    pub(super) fn request_new_remote_folder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.remote_transfer_working() || self.remote_browser.preflighting {
            return;
        }
        let Some(target) = self.current_remote_transfer_target() else { return };
        let language = crate::gpui_shell::config::ui_language(cx);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(language.text(Message::RemoteNewFolderPlaceholder))
        });
        let focus_input = input.clone();
        let workspace = cx.entity().downgrade();
        let parent_display = target.path.clone();
        window.open_dialog(cx, move |dialog, window, _cx| {
            let submit_input = input.clone();
            let workspace = workspace.clone();
            let target = target.clone();
            confirm_dialog(
                dialog,
                window,
                language.text(Message::RemoteNewFolderTitle),
                parent_display.clone(),
                language.text(Message::CommonConfirm),
                language.text(Message::CommonCancel),
                ButtonVariant::Primary,
            )
            .child(div().w_full().pt_1().child(Input::new(&input).w_full()))
            .on_ok(move |_, window, cx| {
                let folder_name = submit_input.read(cx).value().trim().to_owned();
                if let Err(message) = crate::ssh_sftp::validate_name(&folder_name) {
                    crate::gpui_shell::toast::toast(
                        window,
                        cx,
                        crate::display::ToastKind::Warning,
                        message,
                    );
                    return false;
                }
                if let Some(workspace) = workspace.upgrade() {
                    let _ = workspace.update(cx, |this, cx| {
                        this.execute_remote_mkdir(&folder_name, &target, cx);
                    });
                }
                true
            })
        });
        focus_input.update(cx, |state, cx| state.focus(window, cx));
    }

    fn execute_remote_mkdir(
        &mut self,
        name: &str,
        target: &RemoteTransferTarget,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.remote_transfer_target_matches(target) {
            self.remote_browser.error =
                Some(workspace_ui_language().text(Message::TransferTargetChanged).to_owned());
            cx.notify();
            return;
        }
        match self.remote_transfer_controller(target, cx) {
            Ok(controller) => {
                self.remote_browser.error = None;
                if let Err(message) = controller.create_directory(name) {
                    self.remote_browser.error = Some(message);
                }
                cx.notify();
            },
            Err(message) => {
                self.remote_browser.error = Some(message);
                cx.notify();
            },
        }
    }
}

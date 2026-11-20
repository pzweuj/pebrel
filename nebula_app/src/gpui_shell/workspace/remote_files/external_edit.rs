//! Open remote files in the OS default application, watch the managed local temp
//! copy for saves, and prompt (or auto-upload) to sync changes back to the server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpui_component::dialog::CancelDialog;

use super::*;
use crate::i18n::Message;
use crate::ssh_sftp::external_edit::{
    ExternalEditFile, download_for_external_edit, file_fingerprint, upload_external_edit,
};

const POLL_INTERVAL: Duration = Duration::from_millis(600);
const SETTLE_DELAY: Duration = Duration::from_millis(250);

#[derive(Debug, PartialEq, Eq)]
enum ChangeAction {
    None,
    Prompt,
    AutoUpload,
}

struct ExternalEditSession {
    id: u64,
    destination: String,
    remote_path: String,
    canonical_remote_path: String,
    name: String,
    local_path: PathBuf,
    temp_dir: PathBuf,
    permissions: Option<u32>,
    last_synced_hash: [u8; 32],
    last_prompted_hash: [u8; 32],
    last_observed_meta: Option<(u64, Option<SystemTime>)>,
    auto_upload: bool,
    prompt_open: bool,
    uploading: bool,
    watcher: Option<gpui::Task<()>>,
}

impl ExternalEditSession {
    fn decide_change(
        &mut self,
        new_hash: [u8; 32],
        new_size: u64,
        new_modified: Option<SystemTime>,
    ) -> ChangeAction {
        self.last_observed_meta = Some((new_size, new_modified));
        if self.prompt_open || self.uploading {
            return ChangeAction::None;
        }
        if new_hash == self.last_synced_hash {
            return ChangeAction::None;
        }
        if self.auto_upload {
            return ChangeAction::AutoUpload;
        }
        if new_hash == self.last_prompted_hash {
            return ChangeAction::None;
        }
        self.prompt_open = true;
        self.last_prompted_hash = new_hash;
        ChangeAction::Prompt
    }
}

impl Drop for ExternalEditSession {
    fn drop(&mut self) {
        self.watcher = None;
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

#[derive(Default)]
pub(super) struct ExternalEditManager {
    next_id: u64,
    sessions: HashMap<u64, ExternalEditSession>,
}

impl ExternalEditManager {
    fn allocate_id(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    fn find_by_remote(&self, destination: &str, remote_path: &str) -> Option<u64> {
        self.sessions
            .values()
            .find(|session| {
                session.destination == destination && session.remote_path == remote_path
            })
            .map(|session| session.id)
    }

    fn temp_dir_for(&self, id: u64) -> PathBuf {
        std::env::temp_dir()
            .join("pebrel-sftp-edit")
            .join(format!("pid-{}-{id}", std::process::id()))
    }
}

impl NebulaWorkspace {
    /// Download a remote file into a managed temporary folder, launch it with the
    /// system default application, and monitor it for saves to offer uploading back.
    pub(super) fn open_remote_file_with_default_app(
        &mut self,
        entry: SftpEntry,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if entry.is_parent || entry.kind == SftpEntryKind::Directory {
            return;
        }
        let destination = self.remote_browser.destination.clone();
        if destination.is_empty() {
            return;
        }
        let language = crate::gpui_shell::config::ui_language(cx);

        // If this remote file is already actively watched with a local copy on disk,
        // reopen the existing copy so unsaved/un-uploaded edits are never clobbered.
        if let Some(existing_id) =
            self.remote_browser.external_edits.find_by_remote(&destination, &entry.path)
            && let Some(session) = self.remote_browser.external_edits.sessions.get(&existing_id)
            && session.local_path.is_file()
        {
            let local_path = session.local_path.clone();
            let name = session.name.clone();
            match crate::platform::file_manager::open(&local_path) {
                Ok(()) => {
                    crate::gpui_shell::toast::toast(
                        window,
                        cx,
                        crate::display::ToastKind::Info,
                        language.format(Message::RemoteOpenWithDefaultWatching, &[("name", &name)]),
                    );
                },
                Err(error) => {
                    crate::gpui_shell::toast::toast(
                        window,
                        cx,
                        crate::display::ToastKind::Warning,
                        language.format(
                            Message::RemoteOpenWithDefaultFailed,
                            &[("name", &name), ("error", &error.to_string())],
                        ),
                    );
                },
            }
            return;
        }

        let edit_id = self.remote_browser.external_edits.allocate_id();
        let temp_dir = self.remote_browser.external_edits.temp_dir_for(edit_id);
        let remote_path = entry.path.clone();
        let display_name = entry.name.clone();

        crate::gpui_shell::toast::toast(
            window,
            cx,
            crate::display::ToastKind::Info,
            language.format(Message::RemoteOpenWithDefaultDownloading, &[("name", &display_name)]),
        );

        let download_destination = destination.clone();
        let download_remote = remote_path.clone();
        let download_temp = temp_dir.clone();
        cx.spawn_in(window, async move |this, cx| {
            let downloaded = remote_call(move || async move {
                download_for_external_edit(&download_destination, &download_remote, &download_temp)
                    .await
            })
            .await;

            let _ = this.update_in(cx, |workspace, window, cx| {
                let language = crate::gpui_shell::config::ui_language(cx);
                let file = match downloaded {
                    Some(Ok(file)) => file,
                    Some(Err(error)) => {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        crate::gpui_shell::toast::toast(
                            window,
                            cx,
                            crate::display::ToastKind::Warning,
                            language.format(
                                Message::RemoteOpenWithDefaultFailed,
                                &[("name", &display_name), ("error", &error)],
                            ),
                        );
                        return;
                    },
                    None => {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        crate::gpui_shell::toast::toast(
                            window,
                            cx,
                            crate::display::ToastKind::Warning,
                            language.format(
                                Message::RemoteOpenWithDefaultFailed,
                                &[
                                    ("name", &display_name),
                                    (
                                        "error",
                                        language.text(Message::FilesRemoteConnectionUnavailable),
                                    ),
                                ],
                            ),
                        );
                        return;
                    },
                };

                if let Err(error) = crate::platform::file_manager::open(&file.local_path) {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    crate::gpui_shell::toast::toast(
                        window,
                        cx,
                        crate::display::ToastKind::Warning,
                        language.format(
                            Message::RemoteOpenWithDefaultFailed,
                            &[("name", &file.name), ("error", &error.to_string())],
                        ),
                    );
                    return;
                }

                let opened_name = file.name.clone();
                workspace.register_external_edit_session(edit_id, temp_dir, file, window, cx);
                crate::gpui_shell::toast::toast(
                    window,
                    cx,
                    crate::display::ToastKind::Info,
                    language
                        .format(Message::RemoteOpenWithDefaultWatching, &[("name", &opened_name)]),
                );
            });
        })
        .detach();
    }

    fn register_external_edit_session(
        &mut self,
        id: u64,
        temp_dir: PathBuf,
        file: ExternalEditFile,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let local_path = file.local_path.clone();
        let initial_meta = Some((file.size, file.modified));
        let session = ExternalEditSession {
            id,
            destination: file.destination,
            remote_path: file.remote_path,
            canonical_remote_path: file.canonical_remote_path,
            name: file.name,
            local_path: file.local_path,
            temp_dir,
            permissions: file.permissions,
            last_synced_hash: file.sha256,
            last_prompted_hash: file.sha256,
            last_observed_meta: initial_meta,
            auto_upload: false,
            prompt_open: false,
            uploading: false,
            watcher: None,
        };
        self.remote_browser.external_edits.sessions.insert(id, session);

        let executor = cx.background_executor().clone();
        let watcher = cx.spawn_in(window, async move |this, cx| {
            loop {
                executor.timer(POLL_INTERVAL).await;
                let state = this
                    .update(cx, |workspace, _| {
                        workspace.remote_browser.external_edits.sessions.get(&id).map(|session| {
                            (session.prompt_open || session.uploading, session.last_observed_meta)
                        })
                    })
                    .ok()
                    .flatten();
                let Some((paused, observed)) = state else {
                    break;
                };
                if paused {
                    continue;
                }
                let probe_path = local_path.clone();
                let current_meta = executor
                    .spawn(async move {
                        let meta = std::fs::metadata(&probe_path).ok()?;
                        meta.is_file().then(|| (meta.len(), meta.modified().ok()))
                    })
                    .await;
                let Some(current_meta) = current_meta else {
                    continue;
                };
                if observed == Some(current_meta) {
                    continue;
                }
                // Wait briefly so editors that truncate-then-write or atomic-rename finish.
                executor.timer(SETTLE_DELAY).await;
                let settle_path = local_path.clone();
                let settled = executor
                    .spawn(async move {
                        let meta = std::fs::metadata(&settle_path).ok()?;
                        if !meta.is_file() {
                            return None;
                        }
                        let after = (meta.len(), meta.modified().ok());
                        if after != current_meta {
                            return None;
                        }
                        file_fingerprint(&settle_path).ok()
                    })
                    .await;
                let Some((new_hash, new_size, new_modified)) = settled else {
                    continue;
                };
                let alive = this
                    .update_in(cx, |workspace, window, cx| {
                        workspace.handle_external_edit_change(
                            id,
                            new_hash,
                            new_size,
                            new_modified,
                            window,
                            cx,
                        );
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        if let Some(session) = self.remote_browser.external_edits.sessions.get_mut(&id) {
            session.watcher = Some(watcher);
        }
    }

    fn handle_external_edit_change(
        &mut self,
        id: u64,
        new_hash: [u8; 32],
        new_size: u64,
        new_modified: Option<SystemTime>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(session) = self.remote_browser.external_edits.sessions.get_mut(&id) else {
            return;
        };
        match session.decide_change(new_hash, new_size, new_modified) {
            ChangeAction::None => {},
            ChangeAction::AutoUpload => {
                self.upload_external_edit_session(id, window, cx);
            },
            ChangeAction::Prompt => {
                self.open_external_edit_sync_dialog(id, new_size, window, cx);
            },
        }
    }

    fn open_external_edit_sync_dialog(
        &mut self,
        id: u64,
        new_size: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(session) = self.remote_browser.external_edits.sessions.get(&id) else {
            return;
        };
        let name = session.name.clone();
        let destination = session.destination.clone();
        let remote_path = session.remote_path.clone();
        let size_label = format_transfer_bytes(new_size);
        let language = workspace_ui_language();
        let workspace = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, window, _cx| {
            let ignore_workspace = workspace.clone();
            let always_workspace = workspace.clone();
            let upload_workspace = workspace.clone();
            let cancel_workspace = workspace.clone();

            let footer = DialogFooter::new()
                .child(
                    Button::new("sftp-external-sync-ignore")
                        .label(language.text(Message::RemoteExternalSyncIgnore))
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = ignore_workspace.upgrade() {
                                let _ = workspace.update(cx, |workspace, _| {
                                    if let Some(session) = workspace
                                        .remote_browser
                                        .external_edits
                                        .sessions
                                        .get_mut(&id)
                                    {
                                        session.prompt_open = false;
                                    }
                                });
                            }
                            window.dispatch_action(Box::new(CancelDialog), cx);
                        }),
                )
                .child(div().flex_1())
                .child(
                    Button::new("sftp-external-sync-always")
                        .label(language.text(Message::RemoteExternalSyncAlways))
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = always_workspace.upgrade() {
                                let _ = workspace.update(cx, |workspace, cx| {
                                    if let Some(session) = workspace
                                        .remote_browser
                                        .external_edits
                                        .sessions
                                        .get_mut(&id)
                                    {
                                        session.prompt_open = false;
                                        session.auto_upload = true;
                                    }
                                    workspace.upload_external_edit_session(id, window, cx);
                                });
                            }
                            window.dispatch_action(Box::new(CancelDialog), cx);
                        }),
                )
                .child(
                    Button::new("sftp-external-sync-upload")
                        .label(language.text(Message::RemoteExternalSyncUpload))
                        .primary()
                        .on_click(move |_, window, cx| {
                            if let Some(workspace) = upload_workspace.upgrade() {
                                let _ = workspace.update(cx, |workspace, cx| {
                                    if let Some(session) = workspace
                                        .remote_browser
                                        .external_edits
                                        .sessions
                                        .get_mut(&id)
                                    {
                                        session.prompt_open = false;
                                    }
                                    workspace.upload_external_edit_session(id, window, cx);
                                });
                            }
                            window.dispatch_action(Box::new(CancelDialog), cx);
                        }),
                );

            let body = language.format(
                Message::RemoteExternalSyncBody,
                &[
                    ("name", &name),
                    ("size", &size_label),
                    ("destination", &destination),
                    ("path", &remote_path),
                ],
            );

            center_modal_dialog(dialog, window, 190.0)
                .close_button(false)
                .overlay_closable(true)
                .on_cancel(move |_, _, cx| {
                    if let Some(workspace) = cancel_workspace.upgrade() {
                        let _ = workspace.update(cx, |workspace, _| {
                            if let Some(session) =
                                workspace.remote_browser.external_edits.sessions.get_mut(&id)
                            {
                                session.prompt_open = false;
                            }
                        });
                    }
                    true
                })
                .title(
                    div()
                        .text_lg()
                        .font_semibold()
                        .child(language.text(Message::RemoteExternalSyncTitle)),
                )
                .footer(footer)
                .child(div().w_full().text_sm().child(body))
        });
    }

    fn upload_external_edit_session(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(session) = self.remote_browser.external_edits.sessions.get_mut(&id) else {
            return;
        };
        if session.uploading {
            return;
        }
        session.uploading = true;
        let destination = session.destination.clone();
        let canonical = session.canonical_remote_path.clone();
        let remote_path = session.remote_path.clone();
        let local_path = session.local_path.clone();
        let permissions = session.permissions;
        let name = session.name.clone();

        cx.spawn_in(window, async move |this, cx| {
            let uploaded = remote_call(move || async move {
                upload_external_edit(&destination, &canonical, &local_path, permissions).await
            })
            .await;

            let _ = this.update_in(cx, |workspace, window, cx| {
                let language = crate::gpui_shell::config::ui_language(cx);
                let Some(session) = workspace.remote_browser.external_edits.sessions.get_mut(&id)
                else {
                    return;
                };
                session.uploading = false;
                let session_destination = session.destination.clone();
                match uploaded {
                    Some(Ok((hash, size, modified))) => {
                        session.last_synced_hash = hash;
                        session.last_prompted_hash = hash;
                        session.last_observed_meta = Some((size, modified));
                        crate::gpui_shell::toast::toast(
                            window,
                            cx,
                            crate::display::ToastKind::Success,
                            language
                                .format(Message::RemoteExternalSyncUploaded, &[("name", &name)]),
                        );
                        // Refresh the remote listing if the user is still viewing the file's parent dir.
                        let parent = remote_path
                            .rsplit_once('/')
                            .map(|(dir, _)| if dir.is_empty() { "/" } else { dir })
                            .unwrap_or("/");
                        if workspace.remote_browser.destination == session_destination
                            && workspace.remote_browser.path == parent
                            && !workspace.remote_browser.loading
                        {
                            workspace.remote_refresh(cx);
                        }
                    },
                    Some(Err(error)) => {
                        crate::gpui_shell::toast::toast(
                            window,
                            cx,
                            crate::display::ToastKind::Warning,
                            language.format(
                                Message::RemoteExternalSyncFailed,
                                &[("name", &name), ("error", &error)],
                            ),
                        );
                    },
                    None => {
                        crate::gpui_shell::toast::toast(
                            window,
                            cx,
                            crate::display::ToastKind::Warning,
                            language.format(
                                Message::RemoteExternalSyncFailed,
                                &[
                                    ("name", &name),
                                    (
                                        "error",
                                        language.text(Message::FilesRemoteConnectionUnavailable),
                                    ),
                                ],
                            ),
                        );
                    },
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session(temp_dir: PathBuf) -> ExternalEditSession {
        ExternalEditSession {
            id: 1,
            destination: "dev@host".to_owned(),
            remote_path: "/srv/app.conf".to_owned(),
            canonical_remote_path: "/srv/app.conf".to_owned(),
            name: "app.conf".to_owned(),
            local_path: temp_dir.join("app.conf"),
            temp_dir,
            permissions: Some(0o644),
            last_synced_hash: [1u8; 32],
            last_prompted_hash: [1u8; 32],
            last_observed_meta: Some((10, None)),
            auto_upload: false,
            prompt_open: false,
            uploading: false,
            watcher: None,
        }
    }

    #[test]
    fn change_state_machine_deduplicates_prompts_and_honors_auto_upload() {
        let dir = tempfile::tempdir().unwrap().keep();
        let mut session = test_session(dir.clone());

        // Same content as last synced: never prompt.
        assert_eq!(session.decide_change([1u8; 32], 10, None), ChangeAction::None);

        // First real edit: prompt once and mark prompt open.
        assert_eq!(session.decide_change([2u8; 32], 12, None), ChangeAction::Prompt);
        assert!(session.prompt_open);
        assert_eq!(session.decide_change([3u8; 32], 14, None), ChangeAction::None);

        // User ignores hash [2]: same hash must not re-prompt, but a newer edit [3] will.
        session.prompt_open = false;
        assert_eq!(session.decide_change([2u8; 32], 12, None), ChangeAction::None);
        assert_eq!(session.decide_change([3u8; 32], 14, None), ChangeAction::Prompt);

        // Once auto_upload is enabled, subsequent distinct edits upload without prompting.
        session.prompt_open = false;
        session.auto_upload = true;
        assert_eq!(session.decide_change([4u8; 32], 16, None), ChangeAction::AutoUpload);

        // Dropping the session cleans up its managed temporary directory.
        drop(session);
        assert!(!dir.exists());
    }
}

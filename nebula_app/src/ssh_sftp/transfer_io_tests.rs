//! Exercise transfer integrity and cancellation through the real SFTP codec.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use russh_sftp::protocol::{
    Data, ExtendedReply, FileAttributes, Handle, Packet, Status, StatusCode, Version,
};

use super::*;

#[derive(Default)]
struct Progress {
    bytes: AtomicU64,
    cancelled: AtomicBool,
}

impl TransferObserver for Progress {
    fn advance(&self, bytes: u64) {
        self.bytes.fetch_add(bytes, Ordering::SeqCst);
    }
    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Files {
    data: Vec<u8>,
    short_reads: bool,
    reject_writes: bool,
    max_open_handles: Option<usize>,
    replacement_on_failure: Option<Vec<u8>>,
    fail_once_read_at: Option<u64>,
    cancel_after_read: Option<Arc<Progress>>,
}

struct Peer {
    files: Arc<std::sync::Mutex<Files>>,
    handles: BTreeSet<String>,
    replaced_file: Option<(BTreeSet<String>, Vec<u8>)>,
    advertised_limit: Option<u64>,
}

fn success(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}

impl russh_sftp::server::Handler for Peer {
    type Error = StatusCode;
    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _: u32, _: HashMap<String, String>) -> Result<Version, StatusCode> {
        let mut version = Version::new();
        if self.advertised_limit.is_some() {
            version.extensions.insert("limits@openssh.com".into(), "1".into());
        }
        Ok(version)
    }

    async fn extended(
        &mut self,
        id: u32,
        request: String,
        _: Vec<u8>,
    ) -> Result<Packet, StatusCode> {
        if request != "limits@openssh.com" {
            return Err(StatusCode::OpUnsupported);
        }
        let limit = self.advertised_limit.ok_or(StatusCode::OpUnsupported)?;
        let data = [0, 0, 0, limit].into_iter().flat_map(u64::to_be_bytes).collect();
        Ok(ExtendedReply { id, data }.into())
    }

    async fn open(
        &mut self,
        id: u32,
        _: String,
        flags: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, StatusCode> {
        {
            let mut files = self.files.lock().unwrap();
            if files.max_open_handles.is_some_and(|limit| self.handles.len() >= limit) {
                return Err(StatusCode::Failure);
            }
            if flags.contains(OpenFlags::TRUNCATE) {
                files.data.clear();
            }
        }
        let handle = id.to_string();
        self.handles.insert(handle.clone());
        Ok(Handle { id, handle })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        length: u32,
    ) -> Result<Data, StatusCode> {
        if !self.handles.contains(&handle) {
            return Err(StatusCode::Failure);
        }
        assert!(length as usize <= TRANSFER_CHUNK, "local buffering must not grow packets");
        let mut files = self.files.lock().unwrap();
        if let Some(progress) = files.cancel_after_read.take() {
            progress.cancelled.store(true, Ordering::SeqCst);
        }
        if files.fail_once_read_at == Some(offset) {
            files.fail_once_read_at = None;
            if let Some(replacement) = files.replacement_on_failure.take() {
                let original = std::mem::replace(&mut files.data, replacement);
                self.replaced_file = Some((self.handles.clone(), original));
            }
            return Err(StatusCode::Failure);
        }
        let offset = offset as usize;
        let data = match &self.replaced_file {
            Some((original_handles, original)) if original_handles.contains(&handle) => original,
            _ => &files.data,
        };
        if offset >= data.len() {
            return Err(StatusCode::Eof);
        }
        let length = if files.short_reads { (length as usize).min(1013) } else { length as usize };
        let end = (offset + length).min(data.len());
        Ok(Data { id, data: data[offset..end].to_vec() })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, StatusCode> {
        if !self.handles.contains(&handle) {
            return Err(StatusCode::Failure);
        }
        assert!(data.len() <= TRANSFER_CHUNK);
        let mut files = self.files.lock().unwrap();
        if files.reject_writes {
            return Err(StatusCode::PermissionDenied);
        }
        let start = offset as usize;
        let end = start + data.len();
        if end > files.data.len() {
            files.data.resize(end, 0);
        }
        files.data[start..end].copy_from_slice(&data);
        Ok(success(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        self.handles.remove(&handle);
        Ok(success(id))
    }
}

async fn peer() -> (Arc<std::sync::Mutex<Files>>, SftpSession) {
    peer_with_limit(None).await
}

async fn peer_with_limit(
    advertised_limit: Option<u64>,
) -> (Arc<std::sync::Mutex<Files>>, SftpSession) {
    let files = Arc::new(std::sync::Mutex::new(Files::default()));
    let (client, server) = tokio::io::duplex(128 * 1024);
    tokio::spawn(russh_sftp::server::run(
        server,
        Peer {
            files: files.clone(),
            handles: BTreeSet::new(),
            replaced_file: None,
            advertised_limit,
        },
    ));
    let session = SftpSession::new_with_config(client, crate::ssh_sftp::limits::session_config())
        .await
        .unwrap();
    (files, session)
}

fn run(future: impl Future<Output = ()>) {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), future).await.unwrap()
        });
}

#[test]
fn buffered_transfers_preserve_empty_short_and_segmented_files() {
    run(async {
        let (files, session) = peer().await;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        for size in [0, 7, TRANSFER_CHUNK + 17, 2 * 1024 * 1024 + 123] {
            let data: Vec<u8> = (0..size).map(|i| ((i * 37 + i / 113) % 251) as u8).collect();
            std::fs::write(&source, &data).unwrap();
            let uploaded = Progress::default();
            upload_stream(&session, &source, "remote", &uploaded).await.unwrap();
            assert_eq!(files.lock().unwrap().data, data);
            assert_eq!(uploaded.bytes.load(Ordering::SeqCst), size as u64);
            files.lock().unwrap().short_reads = true;
            let downloaded = Progress::default();
            let result =
                download_segmented(&session, "remote", size as u64, &target, 4, &downloaded)
                    .await
                    .unwrap();
            assert_eq!((result.written, result.contiguous), (size as u64, size as u64));
            assert_eq!(downloaded.bytes.load(Ordering::SeqCst), size as u64);
            assert_eq!(std::fs::read(&target).unwrap(), data);
        }
        session.close().await.unwrap();
    });
}

#[test]
fn early_eof_never_reports_a_complete_download() {
    run(async {
        let (files, session) = peer_with_limit(Some(4)).await;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        files.lock().unwrap().data = vec![42; TRANSFER_CHUNK + 17];
        for window in [1, 4, 64] {
            let downloaded = Progress::default();
            let result = download_segmented(
                &session,
                "remote",
                2 * LOCAL_IO_BUFFER as u64,
                &target,
                window,
                &downloaded,
            )
            .await;
            let error = result.err().expect("a short source must not be published as success");
            assert_eq!(
                error.downcast_ref::<io::Error>().unwrap().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
        assert_full_quota(&session, 4).await;
        session.close().await.unwrap();
    });
}

#[test]
fn cancellation_does_not_count_data_waiting_in_the_local_buffer() {
    run(async {
        let (files, session) = peer_with_limit(Some(4)).await;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let progress = Arc::new(Progress::default());
        {
            let mut files = files.lock().unwrap();
            files.data = vec![42; 2 * LOCAL_IO_BUFFER];
            files.cancel_after_read = Some(progress.clone());
        }
        let result = download_segmented(
            &session,
            "remote",
            2 * LOCAL_IO_BUFFER as u64,
            &target,
            1,
            progress.as_ref(),
        )
        .await;
        let error = result.err().expect("cancelled transfer must fail");
        assert_eq!(error.downcast_ref::<io::Error>().unwrap().kind(), io::ErrorKind::Interrupted);
        assert_eq!(progress.bytes.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::metadata(&target).unwrap().len(), 0);
        assert_full_quota(&session, 4).await;
        session.close().await.unwrap();
    });
}

#[test]
fn buffered_upload_still_propagates_server_write_errors() {
    run(async {
        let (files, session) = peer().await;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        std::fs::write(&source, [42; TRANSFER_CHUNK]).unwrap();
        files.lock().unwrap().reject_writes = true;
        assert!(upload_stream(&session, &source, "remote", &Progress::default()).await.is_err());
        assert!(files.lock().unwrap().data.is_empty());
        session.close().await.unwrap();
    });
}

#[test]
fn handle_limited_server_falls_back_to_single_stream_download() {
    run(async {
        let (files, session) = peer().await;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("fallback.bin");
        let size = 4 * TRANSFER_CHUNK + 519;
        let data: Vec<u8> = (0..size).map(|i| ((i * 29 + 7) % 251) as u8).collect();
        {
            let mut guard = files.lock().unwrap();
            guard.data = data.clone();
            guard.max_open_handles = Some(2);
        }
        let downloaded = Progress::default();
        let outcome = download_segmented(&session, "remote", size as u64, &target, 16, &downloaded)
            .await
            .unwrap();
        assert_eq!((outcome.written, outcome.contiguous), (size as u64, size as u64));
        assert_eq!(downloaded.bytes.load(Ordering::SeqCst), size as u64);
        assert_eq!(std::fs::read(&target).unwrap(), data);
        session.close().await.unwrap();
    });
}

#[test]
fn download_segmented_closes_local_handle_and_applies_mtime_before_returning() {
    run(async {
        let (files, session) = peer().await;
        let directory = tempfile::tempdir().unwrap();
        let staging = directory.path().join(".report.bin.nebula-download-test");
        let published = directory.path().join("report.bin");
        let size = 3 * TRANSFER_CHUNK + 101;
        let data: Vec<u8> = (0..size).map(|i| ((i * 17 + 3) % 251) as u8).collect();
        files.lock().unwrap().data = data.clone();

        let modified = 1_700_000_123u64;
        let downloaded = Progress::default();
        let outcome = download_segmented_with_times(
            &session,
            "remote",
            size as u64,
            &staging,
            8,
            modified,
            &downloaded,
        )
        .await
        .unwrap();
        assert_eq!((outcome.written, outcome.contiguous), (size as u64, size as u64));

        // Immediately rename and verify exclusive access is free on Windows.
        std::fs::rename(&staging, &published).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            let exclusive =
                std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(&published);
            assert!(exclusive.is_ok(), "download must close its local handle before returning");
        }
        let mtime = std::fs::metadata(&published)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(mtime, modified);
        assert_eq!(std::fs::read(&published).unwrap(), data);
        session.close().await.unwrap();
    });
}

#[test]
fn segment_read_failure_recovers_sequentially_without_double_counting_progress() {
    run(async {
        let (files, session) = peer().await;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("recovered.bin");
        // Use a multi-batch segment size so the failing segment has already
        // flushed at least one LOCAL_IO_BUFFER batch before failing mid-range.
        let size = 4 * LOCAL_IO_BUFFER + 12_345;
        let data: Vec<u8> = (0..size).map(|i| ((i * 41 + i / 97) % 251) as u8).collect();
        let plan = plan_segments(size as u64, 4);
        assert_eq!(plan.len(), 4);
        let fail_offset = plan[1].0 + LOCAL_IO_BUFFER as u64;
        {
            let mut guard = files.lock().unwrap();
            guard.data = data.clone();
            guard.fail_once_read_at = Some(fail_offset);
            guard.replacement_on_failure = Some(vec![0xff; size]);
        }

        let downloaded = Progress::default();
        let outcome = download_segmented(&session, "remote", size as u64, &target, 4, &downloaded)
            .await
            .unwrap();
        assert_eq!((outcome.written, outcome.contiguous), (size as u64, size as u64));
        assert_eq!(downloaded.bytes.load(Ordering::SeqCst), size as u64);
        assert_eq!(std::fs::read(&target).unwrap(), data);
        assert!(files.lock().unwrap().replacement_on_failure.is_none());
        session.close().await.unwrap();
    });
}

#[test]
fn repeated_downloads_close_remote_handles_cleanly() {
    run(async {
        let (files, session) = peer_with_limit(Some(4)).await;
        let directory = tempfile::tempdir().unwrap();
        let size = 4 * TRANSFER_CHUNK;
        let data: Vec<u8> = (0..size).map(|i| ((i * 19 + 5) % 251) as u8).collect();
        {
            let mut guard = files.lock().unwrap();
            guard.data = data.clone();
            // Exactly fits one 4-segment download; leaked handles would force
            // later downloads to fail or degrade.
            guard.max_open_handles = Some(4);
        }

        for round in 0..3 {
            let target = directory.path().join(format!("round-{round}.bin"));
            let downloaded = Progress::default();
            let outcome =
                download_segmented(&session, "remote", size as u64, &target, 4, &downloaded)
                    .await
                    .unwrap();
            assert_eq!((outcome.written, outcome.contiguous), (size as u64, size as u64));
            assert_eq!(std::fs::read(&target).unwrap(), data);
            assert_full_quota(&session, 4).await;
        }
        session.close().await.unwrap();
    });
}

async fn assert_full_quota(session: &SftpSession, limit: usize) {
    let mut handles = Vec::new();
    for _ in 0..limit {
        handles.push(session.open("remote").await.expect("handle quota should be restored"));
    }
    assert!(matches!(
        session.open("remote").await,
        Err(russh_sftp::client::error::Error::Limited(_))
    ));
    for mut handle in handles {
        handle.shutdown().await.unwrap();
    }
}

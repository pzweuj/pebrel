use super::*;

pub(super) struct Peer;
impl russh_sftp::server::Handler for Peer {
    type Error = russh_sftp::protocol::StatusCode;
    fn unimplemented(&self) -> Self::Error {
        russh_sftp::protocol::StatusCode::OpUnsupported
    }
}

#[test]
fn rejected_sftp_reports_the_subsystem_and_releases_the_channel() {
    check(async {
        let mut fixture = Fixture::new(Mode::RejectSftp).await;
        let acquired = fixture.connect().await;
        let mut shell = own_channel(acquired.session.channel_open_session().await.unwrap());
        for _ in 0..3 {
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                super::super::super::sftp_on_session(&acquired.session),
            )
            .await;
            let error = result.expect("a rejected subsystem must fail immediately").err().unwrap();
            assert!(error.to_string().contains("rejected the SFTP subsystem"), "{error}");
            assert_eq!(fixture.data.recv().await.unwrap(), b"sftp-request");
            assert_eq!(fixture.data.recv().await.unwrap(), b"closed");
        }
        assert!(!acquired.session.is_closed());
        shell.finish().await.unwrap();
        fixture.forget(&acquired.session).await;
    });
}

#[test]
fn cancelling_sftp_negotiation_closes_only_its_channel() {
    check(async {
        let mut fixture = Fixture::new(Mode::HangSftp).await;
        let acquired = fixture.connect().await;
        let mut shell = own_channel(acquired.session.channel_open_session().await.unwrap());
        let mut work = Box::pin(super::super::super::sftp_on_session(&acquired.session));
        tokio::select! {
            _ = &mut work => panic!("fixture withholds the subsystem reply"),
            request = fixture.data.recv() => assert_eq!(request.unwrap(), b"sftp-request"),
        }
        drop(work);
        assert_eq!(fixture.data.recv().await.unwrap(), b"closed");
        assert!(!acquired.session.is_closed());
        shell.finish().await.unwrap();
        fixture.forget(&acquired.session).await;
    });
}

#[test]
fn accepted_sftp_can_reopen_on_the_original_pane_transport() {
    check(async {
        let fixture = Fixture::new(Mode::AcceptSftp).await;
        let acquired = fixture.connect().await;
        let (scope, pane) =
            super::super::super::pane_connection::PaneConnectionScope::new(&acquired.session);
        for _ in 0..3 {
            let sftp = pane.open_sftp().await.unwrap();
            sftp.close().await.unwrap();
        }
        drop(scope);
        assert!(pane.open_sftp().await.is_err());
        assert!(!acquired.session.is_closed());
        fixture.forget(&acquired.session).await;
    });
}

#[test]
fn saturated_pooled_session_reconnects_for_sftp_without_closing_existing_shell() {
    check(async {
        let fixture = Fixture::new(Mode::SaturateFirstConnectionSftp).await;
        let first = fixture.connect().await;
        assert!(!first.reused);
        let mut shell = own_channel(first.session.channel_open_session().await.unwrap());

        // The first pooled connection now rejects additional channels (MaxSessions).
        // `open_sftp_on_route` must evict only the pool entry, reconnect, and open SFTP.
        let sftp = super::super::super::open_sftp_on_route(&fixture.route).await.unwrap();
        sftp.close().await.unwrap();

        let second = fixture.connect().await;
        assert!(second.reused, "fresh SFTP transport should now be pooled");
        assert!(!Arc::ptr_eq(&first.session, &second.session));
        assert!(!first.session.is_closed(), "existing shell session must remain alive");

        shell.finish().await.unwrap();
        fixture.forget(&second.session).await;
    });
}

#[test]
fn pooled_subsystem_rejection_does_not_evict_or_reconnect() {
    check(async {
        let mut fixture = Fixture::new(Mode::RejectSftp).await;
        let first = fixture.connect().await;
        assert!(!first.reused);

        let error = super::super::super::open_sftp_on_route(&fixture.route).await.err().unwrap();
        assert!(error.to_string().contains("rejected the SFTP subsystem"), "{error}");
        assert_eq!(fixture.data.recv().await.unwrap(), b"sftp-request");
        assert_eq!(fixture.data.recv().await.unwrap(), b"closed");

        let after = fixture.connect().await;
        assert!(after.reused);
        assert!(Arc::ptr_eq(&first.session, &after.session));
        fixture.forget(&first.session).await;
    });
}

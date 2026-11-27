fn check_channel_starts() {
    use ssh::channel::SessionCommand;

    for (kind, argument) in [
        (b"shell".as_slice(), None),
        (b"exec".as_slice(), Some(b"test command".as_slice())),
        (b"subsystem".as_slice(), Some(b"sftp".as_slice())),
    ] {
        let label = std::str::from_utf8(kind).unwrap();
        let mut start = request(SSH_MSG_CHANNEL_REQUEST, kind, true);
        if let Some(argument) = argument {
            SshBuf::put_string(&mut start, argument);
        }
        let mut session = ServerSession {
            channel: Some(Channel {
                local_id: 0, remote_id: 7, recv_window: 16, send_window: 16,
                eof_received: false, closed: false, pty_term: None, pty_size: (80, 24),
            }),
            io: PacketIo { input: [start.clone(), start].into(), sent: Vec::new() },
        };
        let selected = session.wait_for_channel().unwrap_or_else(|error| {
            panic!("{label} setup failed: {error:?}");
        });
        match selected {
            SessionCommand::Shell => assert_eq!(kind, b"shell"),
            SessionCommand::Exec(command) => {
                assert_eq!(kind, b"exec");
                assert_eq!(Some(command.as_bytes()), argument);
            }
            SessionCommand::Sftp => assert_eq!(kind, b"subsystem"),
        }
        assert!(session.try_recv_data_uncredited().unwrap().is_none());
        assert_eq!(
            session.io.sent,
            [vec![SSH_MSG_CHANNEL_SUCCESS, 0, 0, 0, 7],
             vec![SSH_MSG_CHANNEL_FAILURE, 0, 0, 0, 7]],
            "{label} should succeed during setup and fail once running",
        );
    }
}

use koko_host_windows::parse_frame;

// Hand-written synthetic local protocol inputs for driver validation, not real Mac fixtures.
#[test]
fn rejects_contradictory_approval_and_partial_frames() {
    for line in [
        "Desktop request: 1 {\"action\":\"text\",\"text\":\"a\",\"approved\":true}\n",
        "Desktop request: 1 {\"action\":\"displays\"}",
        "Desktop request: 9 {\"action\":\"audio\"}\n",
        "Desktop request: 3 {\"action\":\"disconnect\"}\n",
    ] {
        assert!(parse_frame(line).is_err());
    }
}

#[test]
fn malformed_known_prefix_is_reported_to_the_supervisor() {
    assert!(parse_frame("Desktop request 17 {}\n").is_err());
}

#[test]
fn carries_agent_policy_and_media_socket_to_the_backend() {
    use koko_host_windows::{Event, Origin};
    for (approval, expected) in [
        ("", Origin::Agent),
        (",\"approved\":true", Origin::ApprovedAgent),
    ] {
        let frame = format!(
            "Desktop request: 9 {{\"action\":\"text\",\"text\":\"two words\",\"actor\":\"agent\"{approval}}}\r\n"
        );
        let Event::Desktop(request) = parse_frame(&frame).unwrap() else {
            panic!("expected desktop request")
        };
        assert_eq!(request.id, 9);
        assert_eq!(request.origin, expected);
    }
    let Event::Desktop(request) = parse_frame(
        "Desktop request: 10 {\"action\":\"audio\",\"video_socket\":\"opaque path\"}\n",
    )
    .unwrap() else {
        panic!("expected audio request")
    };
    assert_eq!(request.video_socket.as_deref(), Some("opaque path"));
}

#[test]
fn preserves_pair_choices_and_encodes_only_one_command() {
    use koko_host_windows::{Command, Event, HelpAnswer};
    use koko_proto::{ControlState, PeerKind, RemoteReply, Scope};
    let Event::PairRequest{id,peer,details}=parse_frame("Pair request: 47 short {\"kind\":\"agent\",\"name\":\"test peer\",\"scope\":\"agent_safe\"}\n").unwrap() else{panic!("expected pair request")};
    assert_eq!(
        (
            id,
            peer.as_str(),
            details.kind,
            details.name.as_str(),
            details.scope
        ),
        (47, "short", PeerKind::Agent, "test peer", Scope::AgentSafe)
    );
    assert_eq!(
        Command::Approve(id, Some(Scope::Observer))
            .encode()
            .unwrap(),
        "approve 47 observer\n"
    );
    assert!(matches!(
        parse_frame("Agent control: awaiting_approval\n").unwrap(),
        Event::AgentControl(ControlState::AwaitingApproval)
    ));
    assert!(
        Command::AgentHelp(HelpAnswer::Resolved, "ok\napproval allow")
            .encode()
            .is_err()
    );
    assert!(
        Command::CustomCode("secret\rapproval allow")
            .encode()
            .is_err()
    );
    assert_eq!(
        Command::DesktopReply(89, &RemoteReply::error("bad\ncommand"))
            .encode()
            .unwrap(),
        "desktop-reply 89 {\"status\":\"error\",\"code\":\"bad\\ncommand\"}\n"
    );
    assert!(!format!("{:?}", parse_frame("Pair invite: secret\n").unwrap()).contains("secret"));
}

#[test]
fn terminal_statuses_reject_trailing_data_and_allow_distinct_extensions() {
    use koko_host_windows::Event;
    for status in [
        "Pair code locked",
        "Custom code rejected",
        "Pair approved",
        "Pair declined",
        "Pair request ended",
        "Session closed",
        "Agent session closed",
        "Outbox refused",
        "Pointer closed",
    ] {
        assert!(!matches!(
            parse_frame(&format!("{status}\n")).unwrap(),
            Event::Other
        ));
        for suffix in [" extra", ": unexpected", "\textra"] {
            assert!(
                parse_frame(&format!("{status}{suffix}\n")).is_err(),
                "{status}{suffix}"
            );
        }
    }
    for line in ["Session metrics: 4\n", "Pointer closedown: complete\n"] {
        assert!(matches!(parse_frame(line).unwrap(), Event::Other));
    }
}

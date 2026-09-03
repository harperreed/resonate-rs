use sendspin::protocol::messages::*;

fn payload(value: serde_json::Value) -> serde_json::Value {
    value["payload"].clone()
}

#[test]
fn envelope_uses_type_and_payload_with_renamed_message() {
    let message = Message::ClientInit(ClientInit {
        client_id: "A".repeat(43),
        version: 1,
        suite: "25519_ChaChaPoly_SHA256".into(),
    });
    let value = serde_json::to_value(&message).unwrap();
    assert_eq!(value["type"], "client/init");
    assert_eq!(value["payload"]["version"], 1);
    assert!(value.get("ClientInit").is_none());
    let decoded: Message = serde_json::from_value(value).unwrap();
    assert!(matches!(
        decoded,
        Message::ClientInit(ClientInit { version: 1, .. })
    ));
}

#[test]
fn cleartext_and_noise_handshake_round_trip() {
    let server_init = ServerInit {
        server_id: "S".repeat(43),
        version: 1,
    };
    let decoded: ServerInit =
        serde_json::from_value(serde_json::to_value(&server_init).unwrap()).unwrap();
    assert_eq!(decoded.server_id, server_init.server_id);

    let noise = Message::NoiseHandshake(NoiseHandshake { data: "abc".into() });
    let value = serde_json::to_value(noise).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"type":"noise/handshake", "payload":{"data":"abc"}})
    );
    assert!(matches!(
        serde_json::from_value::<Message>(value).unwrap(),
        Message::NoiseHandshake(_)
    ));
}

#[test]
fn server_hello_and_client_hello_wire_shapes() {
    let server = Message::ServerHello(ServerHello {
        name: "Living Room".into(),
        languages: Some(vec!["en-US".into()]),
    });
    assert_eq!(
        payload(serde_json::to_value(server).unwrap())["name"],
        "Living Room"
    );

    let hello = ClientHello {
        name: "client".into(),
        device_info: Some(DeviceInfo {
            product_name: Some("Player".into()),
            ..Default::default()
        }),
        supported_roles: vec!["player@v1".into(), "visualizer@v1".into()],
        player_v1_support: None,
        source_v1_support: None,
        visualizer_v1_support: Some(VisualizerV1Support {
            buffer_capacity: 4096,
        }),
        supported_pair_methods: SupportedPairMethods {
            dynamic_pairing_code: Some(DynamicPairingCodeDescriptor {
                out_channels: vec![PairingOutChannel::Display, PairingOutChannel::Speaker],
                formats: vec![PairingCodeFormat::Digits, PairingCodeFormat::QrCode],
                digit_audio: None,
            }),
            ..Default::default()
        },
        unpaired_access: UnpairedAccess { enabled: false },
    };
    let value = serde_json::to_value(&hello).unwrap();
    assert!(value.get("trust_level").is_none());
    assert_eq!(
        value["supported_pair_methods"]["dynamic_pairing_code"]["out_channels"],
        serde_json::json!(["display", "speaker"])
    );
    assert_eq!(value["unpaired_access"]["enabled"], false);
    let decoded: ClientHello = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.name, "client");
}

#[test]
fn server_activate_and_capability_enum_values() {
    let activate = Message::ServerActivate(ServerActivate {
        activities: vec![Activity::Playback, Activity::Pairing],
        active_roles: Some(vec!["player@v1".into()]),
        pairing: Some(ActivatePairing {
            method: PairingMethod::StaticPairingCode,
            format: Some(PairingCodeFormat::Digits),
        }),
    });
    let value = serde_json::to_value(activate).unwrap();
    assert_eq!(value["type"], "server/activate");
    assert_eq!(
        value["payload"]["activities"],
        serde_json::json!(["playback", "pairing"])
    );
    assert_eq!(value["payload"]["pairing"]["method"], "static_pairing_code");

    let artwork = ArtworkState {
        channels: vec![ArtworkChannelConfig {
            source: ArtworkSource::Album,
            format: Some(ImageFormat::Jpeg),
            width: Some(800),
            height: Some(600),
        }],
    };
    let value = serde_json::to_value(artwork).unwrap();
    assert_eq!(value["channels"][0]["source"], "album");
    assert_eq!(value["channels"][0]["format"], "jpeg");
}

#[test]
fn time_messages_round_trip() {
    let client = Message::ClientTime(ClientTime {
        client_transmitted: 123456789,
    });
    assert_eq!(
        payload(serde_json::to_value(client).unwrap())["client_transmitted"],
        123456789
    );
    let server = ServerTime {
        client_transmitted: 1,
        server_received: 2,
        server_transmitted: 3,
    };
    let decoded: ServerTime =
        serde_json::from_value(serde_json::to_value(&server).unwrap()).unwrap();
    assert_eq!(
        (
            decoded.client_transmitted,
            decoded.server_received,
            decoded.server_transmitted
        ),
        (1, 2, 3)
    );
}

#[test]
fn state_optional_fields_and_null_clears_are_distinct() {
    let state = ClientState {
        available: true,
        player: Some(PlayerState {
            volume: Some(100),
            muted: Some(false),
            output_delay_ms: 25,
            supported_commands: vec![PlayerStateCommand::SetOutputDelay],
            ..Default::default()
        }),
        source: None,
        artwork: None,
        visualizer: None,
    };
    let value = serde_json::to_value(&state).unwrap();
    assert_eq!(value["available"], true);
    assert_eq!(
        value["player"]["supported_commands"],
        serde_json::json!(["set_output_delay"])
    );
    assert_eq!(value["player"]["required_lead_time_ms"], 0);
    assert_eq!(value["player"]["min_buffer_ms"], 0);
    let decoded: ClientState = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.player.unwrap().volume, Some(100));

    let server: ServerState =
        serde_json::from_str(r#"{"metadata":null,"controller":null,"color":null}"#).unwrap();
    assert!(matches!(server.metadata, Some(None)));
    assert!(matches!(server.controller, Some(None)));
    assert!(matches!(server.color, Some(None)));
    let absent: ServerState = serde_json::from_str("{}").unwrap();
    assert!(absent.metadata.is_none());
}

#[test]
fn color_state_accepts_rgb_and_rejects_invalid_arrays() {
    let color = ColorState {
        timestamp: 42,
        background_dark: Some([1, 2, 3]),
        background_light: None,
        primary: Some([255, 0, 128]),
        accent: None,
        on_dark: None,
        on_light: None,
    };
    let value = serde_json::to_value(&color).unwrap();
    assert_eq!(value["background_dark"], serde_json::json!([1, 2, 3]));
    let decoded: ColorState = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.primary, Some([255, 0, 128]));
    assert!(serde_json::from_str::<ColorState>(r#"{"timestamp":1,"primary":[1,2]}"#).is_err());
    assert!(serde_json::from_str::<ColorState>(r#"{"timestamp":1,"primary":[1,2,256]}"#).is_err());
}

#[test]
fn metadata_controller_repeat_and_playback_values() {
    let metadata = MetadataState {
        timestamp: 10,
        title: Some("Track".into()),
        artist: Some("Artist".into()),
        album_artist: None,
        album: None,
        artwork_url: None,
        year: Some(2024),
        track: Some(2),
        progress: Some(TrackProgress {
            track_progress: 1000,
            track_duration: 2000,
            playback_speed: 1000,
        }),
    };
    let value = serde_json::to_value(metadata).unwrap();
    assert_eq!(value["progress"]["playback_speed"], 1000);
    let controller = ControllerState {
        supported_commands: vec!["seek".into()],
        volume: 75,
        muted: false,
        repeat: RepeatMode::One,
        shuffle: false,
        seek_max_ms: Some(9000),
    };
    let value = serde_json::to_value(controller).unwrap();
    assert_eq!(value["repeat"], "one");
    assert_eq!(value["seek_max_ms"], 9000);
    let group = GroupUpdate {
        playback_state: PlaybackState::Playing,
        group_id: "g1".into(),
        group_name: "Group".into(),
    };
    assert_eq!(
        serde_json::to_value(group).unwrap()["playback_state"],
        "playing"
    );
}

#[test]
fn server_and_client_commands_round_trip() {
    let server = Message::ServerCommand(ServerCommand {
        player: Some(PlayerCommand {
            command: PlayerCommandType::Volume,
            volume: Some(80),
            mute: None,
            output_delay_ms: None,
        }),
        source: Some(SourceCommand {
            command: SourceCommandType::Start,
        }),
    });
    let value = serde_json::to_value(server).unwrap();
    assert_eq!(value["payload"]["player"]["command"], "volume");
    assert_eq!(value["payload"]["source"]["command"], "start");
    assert!(value["payload"]["player"].get("mute").is_none());
    let client = Message::ClientCommand(ClientCommand {
        controller: Some(ControllerCommand {
            command: ControllerCommandType::Seek,
            volume: None,
            mute: None,
            position_ms: Some(1234),
            offset_ms: None,
        }),
    });
    let value = serde_json::to_value(client).unwrap();
    assert_eq!(value["type"], "client/command");
    assert_eq!(value["payload"]["controller"]["position_ms"], 1234);
    assert!(value["payload"]["controller"].get("offset_ms").is_none());
}

#[test]
fn every_controller_command_has_spec_wire_name() {
    let commands = [
        (ControllerCommandType::Play, "play"),
        (ControllerCommandType::Pause, "pause"),
        (ControllerCommandType::Stop, "stop"),
        (ControllerCommandType::Next, "next"),
        (ControllerCommandType::Previous, "previous"),
        (ControllerCommandType::Volume, "volume"),
        (ControllerCommandType::Mute, "mute"),
        (ControllerCommandType::RepeatOff, "repeat_off"),
        (ControllerCommandType::RepeatOne, "repeat_one"),
        (ControllerCommandType::RepeatAll, "repeat_all"),
        (ControllerCommandType::Shuffle, "shuffle"),
        (ControllerCommandType::Unshuffle, "unshuffle"),
        (ControllerCommandType::Switch, "switch"),
        (ControllerCommandType::Seek, "seek"),
        (ControllerCommandType::SeekRelative, "seek_relative"),
    ];
    for (command, expected) in commands {
        let value = serde_json::to_value(ControllerCommand {
            command: command.clone(),
            volume: None,
            mute: None,
            position_ms: None,
            offset_ms: None,
        })
        .unwrap();
        assert_eq!(value["command"], expected);
        let decoded: ControllerCommand = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.command, command);
    }
}

#[test]
fn stream_start_end_clear_and_source_stream_shapes() {
    let start = Message::StreamStart(StreamStart {
        server_transmitted: 10,
        player: Some(StreamPlayerConfig {
            codec: "opus".into(),
            sample_rate: 48000,
            channels: 2,
            bit_depth: 16,
            codec_header: Some("header".into()),
        }),
        artwork: Some(StreamArtworkConfig {
            channels: vec![ArtworkChannelConfig {
                source: ArtworkSource::Album,
                format: Some(ImageFormat::Png),
                width: Some(320),
                height: Some(240),
            }],
        }),
        visualizer: None,
    });
    let value = serde_json::to_value(start).unwrap();
    assert_eq!(value["type"], "stream/start");
    assert_eq!(value["payload"]["player"]["sample_rate"], 48000);
    assert_eq!(value["payload"]["artwork"]["channels"][0]["format"], "png");
    let end = serde_json::to_value(Message::StreamEnd(StreamEnd {
        roles: Some(vec!["player@v1".into()]),
    }))
    .unwrap();
    assert_eq!(end["type"], "stream/end");
    assert_eq!(end["payload"]["roles"], serde_json::json!(["player@v1"]));
    let clear = serde_json::to_value(Message::StreamClear(StreamClear {
        server_transmitted: 12,
        roles: None,
    }))
    .unwrap();
    assert_eq!(clear["type"], "stream/clear");
    assert!(clear["payload"].get("roles").is_none());
    let source = ClientStreamStart {
        source: SourceStreamConfig {
            codec: "flac".into(),
            channels: 2,
            sample_rate: 44100,
            bit_depth: 24,
            codec_header: None,
        },
    };
    assert_eq!(
        serde_json::to_value(Message::ClientStreamStart(source)).unwrap()["type"],
        "client-stream/start"
    );
    assert_eq!(
        serde_json::to_value(Message::ClientStreamEnd(ClientStreamEnd {})).unwrap(),
        serde_json::json!({"type":"client-stream/end", "payload":{}})
    );
}

#[test]
fn state_visualizer_validator_pins_spectrum_cross_field_rule() {
    let config = SpectrumConfig {
        n_disp_bins: 32,
        scale: SpectrumScale::Log,
        f_min: 20,
        f_max: 20000,
    };
    assert!(VisualizerState {
        types: vec![VisualizerDataType::Spectrum],
        rate_max: 30,
        spectrum: Some(config.clone()),
    }
    .validate()
    .is_ok());
    assert!(VisualizerState {
        types: vec![VisualizerDataType::Spectrum],
        rate_max: 30,
        spectrum: None,
    }
    .validate()
    .is_err());
    assert!(VisualizerState {
        types: vec![VisualizerDataType::Loudness],
        rate_max: 30,
        spectrum: Some(config),
    }
    .validate()
    .is_err());
}

#[test]
fn pair_messages_pin_wire_names_and_round_trip() {
    let messages = vec![
        (
            serde_json::to_value(Message::ClientPairPending(ClientPairPending {
                pairing_index: 2,
            }))
            .unwrap(),
            "client/pair-pending",
        ),
        (
            serde_json::to_value(Message::ClientPairInit(ClientPairInit {
                pairing_index: 2,
                commit_b: Some("commit".into()),
            }))
            .unwrap(),
            "client/pair-init",
        ),
        (
            serde_json::to_value(Message::ServerPairInit(ServerPairInit {
                nonce_a: "nonce".into(),
            }))
            .unwrap(),
            "server/pair-init",
        ),
        (
            serde_json::to_value(Message::ServerPairAuth(ServerPairAuth {
                pake_msg_1: "ya".into(),
            }))
            .unwrap(),
            "server/pair-auth",
        ),
        (
            serde_json::to_value(Message::ClientPairAuth(ClientPairAuth {
                pake_msg_2: "yb".into(),
            }))
            .unwrap(),
            "client/pair-auth",
        ),
        (
            serde_json::to_value(Message::ServerPairConfirm(ServerPairConfirm {
                server_kc: "ta".into(),
            }))
            .unwrap(),
            "server/pair-confirm",
        ),
        (
            serde_json::to_value(Message::ClientPairConfirm(ClientPairConfirm {
                client_kc: "tb".into(),
                wrapped_nonce_b: Some("wrapped".into()),
            }))
            .unwrap(),
            "client/pair-confirm",
        ),
        (
            serde_json::to_value(Message::ClientPairFinalize(ClientPairFinalize {
                long_term_psk: Some("psk".into()),
                wrapped_psk: None,
            }))
            .unwrap(),
            "client/pair-finalize",
        ),
        (
            serde_json::to_value(Message::ServerPairFinalize(ServerPairFinalize {})).unwrap(),
            "server/pair-finalize",
        ),
        (
            serde_json::to_value(Message::PairAbort(PairAbort {
                reason: PairAbortReason::MethodNotSupported,
            }))
            .unwrap(),
            "pair/abort",
        ),
    ];
    for (value, expected_type) in messages {
        assert_eq!(value["type"], expected_type);
        assert!(value.get("payload").is_some());
    }
    let renamed = serde_json::to_value(Message::ServerPairInit(ServerPairInit {
        nonce_a: "n".into(),
    }))
    .unwrap();
    assert_eq!(renamed["payload"]["nonce_A"], "n");
    assert!(renamed["payload"].get("nonce_a").is_none());
}

#[test]
fn required_group_update_fields_are_required_on_wire() {
    assert!(serde_json::from_str::<GroupUpdate>(r#"{"group_id":"g","group_name":"G"}"#).is_err());
    assert!(serde_json::from_str::<GroupUpdate>(
        r#"{"playback_state":"playing","group_name":"G"}"#
    )
    .is_err());
    assert!(
        serde_json::from_str::<GroupUpdate>(r#"{"playback_state":"playing","group_id":"g"}"#)
            .is_err()
    );
}

#[test]
fn goodbye_unpair_and_group_reason_values() {
    for (reason, expected) in [
        (GoodbyeReason::AnotherServer, "another_server"),
        (GoodbyeReason::Shutdown, "shutdown"),
        (GoodbyeReason::Restart, "restart"),
        (GoodbyeReason::UserRequest, "user_request"),
        (GoodbyeReason::Unauthorized, "unauthorized"),
        (GoodbyeReason::PairingRequired, "pairing_required"),
        (GoodbyeReason::ConcurrentAttempt, "concurrent_attempt"),
        (GoodbyeReason::Unpaired, "unpaired"),
    ] {
        let value = serde_json::to_value(Message::ClientGoodbye(ClientGoodbye { reason })).unwrap();
        assert_eq!(value["payload"]["reason"], expected);
        assert!(matches!(
            serde_json::from_value::<Message>(value).unwrap(),
            Message::ClientGoodbye(_)
        ));
    }
    assert_eq!(
        serde_json::to_value(Message::ServerUnpair(ServerUnpair {})).unwrap(),
        serde_json::json!({"type":"server/unpair", "payload":{}})
    );
}

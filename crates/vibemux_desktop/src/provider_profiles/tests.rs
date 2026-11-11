use super::*;

fn profile(id: &str) -> ProviderProfile {
    ProviderProfile {
        id: id.to_owned(),
        name: "Synthetic provider".to_owned(),
        api_type: ProviderApiType::AnthropicMessages,
        base_url: "https://provider.example.test/team/v1".to_owned(),
        model: "operator-model-alias".to_owned(),
        credential_env: Some("SYNTHETIC_PROVIDER_KEY".to_owned()),
        auth_type: ProviderAuthType::ApiKey,
    }
}

fn config() -> ProviderProfilesConfig {
    ProviderProfilesConfig {
        profiles: vec![profile("primary")],
        orchestrator_profile_id: Some("primary".to_owned()),
        ..ProviderProfilesConfig::default()
    }
}

#[test]
fn native_and_ui_validation_share_the_configuration_corpus() {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/provider_validation.json"
    ))
    .expect("shared validation corpus");
    assert_eq!(corpus["schema_version"], 1);
    for case in corpus["cases"].as_array().expect("cases") {
        let mut value = serde_json::to_value(config()).expect("base configuration");
        for (field, replacement) in case["profile"].as_object().expect("profile overrides") {
            value["profiles"][0][field] = replacement.clone();
        }
        assert_eq!(
            ProviderProfilesConfig::from_command_value(value).is_ok(),
            case["valid"].as_bool().expect("expected validity"),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn a_missing_file_is_an_empty_configuration_without_side_effects() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    assert_eq!(
        load_provider_profiles_at(&path),
        Ok(ProviderProfilesConfig::default())
    );
    assert!(!path.exists());
    assert_eq!(fs::read_dir(directory.path()).expect("entries").count(), 0);
}

#[test]
fn multiple_protocols_models_and_versioned_paths_round_trip() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    let mut expected = config();
    expected.profiles.push(ProviderProfile {
        id: "compatible".to_owned(),
        name: "Compatible gateway".to_owned(),
        api_type: ProviderApiType::OpenaiChatCompletions,
        base_url: "https://gateway.example.test/deployment/v2/".to_owned(),
        model: "custom/provider-model".to_owned(),
        credential_env: Some("SYNTHETIC_BEARER_KEY".to_owned()),
        auth_type: ProviderAuthType::Bearer,
    });
    expected.orchestrator_profile_id = Some("compatible".to_owned());
    save_provider_profiles_at(&path, &expected).expect("save");
    assert_eq!(load_provider_profiles_at(&path), Ok(expected.clone()));
    let value = serde_json::to_value(ProviderProfilesView::from(expected)).expect("view");
    assert_eq!(value["readiness"], "configuration_only");
    assert_eq!(
        value["config"]["profiles"][1]["base_url"],
        "https://gateway.example.test/deployment/v2/"
    );
    assert_eq!(fs::read_dir(directory.path()).expect("entries").count(), 1);
}

#[test]
fn profile_selection_never_grants_execution() {
    for id in [None, Some("primary".to_owned())] {
        let mut value = config();
        value.orchestrator_profile_id = id;
        let view = serde_json::to_value(ProviderProfilesView::from(value)).expect("view");
        assert_eq!(view["readiness"], "configuration_only");
        assert_eq!(view.as_object().expect("object").len(), 2);
        assert!(view.get("execution_ready").is_none());
        assert!(view.get("credential_value").is_none());
    }
}

#[test]
fn local_validation_and_save_do_not_contact_the_configured_endpoint() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("owned loopback listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let address = listener.local_addr().expect("address");
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    let mut value = config();
    value.profiles[0].base_url = format!("http://{address}/custom/v1");
    value.profiles[0].credential_env =
        Some(format!("SYNTHETIC_UNSET_{}", uuid::Uuid::new_v4().simple()));
    let decoded =
        ProviderProfilesConfig::from_command_value(serde_json::to_value(&value).expect("request"))
            .expect("metadata does not resolve the credential reference");
    save_provider_profiles_at(&path, &decoded).expect("save");
    assert_eq!(load_provider_profiles_at(&path), Ok(value));
    assert_eq!(
        listener.accept().expect_err("no connection").kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn allowed_urls_keep_explicit_base_paths() {
    for url in [
        "https://api.example.test",
        "https://api.example.test/v1/",
        "https://api.example.test/team/one/v2",
        "https://api.example.test:9443/provider%20space/v1",
        "http://127.0.0.1:11434/v1",
        "http://127.1.2.3:8080/api",
        "http://[::1]:8080/v1",
    ] {
        assert_eq!(validate_base_url(url), Ok(()), "{url}");
    }
}

#[test]
fn unsafe_or_ambiguous_urls_are_refused() {
    for url in [
        "http://api.example.test/v1",
        "http://localhost:8080/v1",
        "http://192.168.1.2:8080/v1",
        "http://[::]:8080/v1",
        "file:///tmp/config",
        "ftp://api.example.test/v1",
        "javascript:alert(1)",
        "//api.example.test/v1",
        "https:api.example.test/v1",
        "https:////api.example.test/v1",
        "https://@api.example.test/v1",
        "https://operator@api.example.test/v1",
        "https://operator:synthetic@api.example.test/v1",
        "https://api.example.test/v1?key=synthetic",
        "https://api.example.test/v1?",
        "https://api.example.test/v1#section",
        "https://api.example.test:0/v1",
        "https://api.example.test:65536/v1",
        "https://api.example.test\\other/v1",
        " https://api.example.test/v1",
        "https://api.example.test/v1\n",
    ] {
        assert_eq!(
            validate_base_url(url),
            Err(ProviderProfilesError::InvalidBaseUrl),
            "{url}"
        );
    }
}

#[test]
fn credential_references_have_explicit_authentication_semantics() {
    let mut value = profile("primary");
    value.auth_type = ProviderAuthType::None;
    assert_eq!(
        value.validate(),
        Err(ProviderProfilesError::InvalidCredentialReference)
    );
    value.credential_env = None;
    assert_eq!(value.validate(), Ok(()));
    for auth in [ProviderAuthType::ApiKey, ProviderAuthType::Bearer] {
        value.auth_type = auth;
        assert_eq!(
            value.validate(),
            Err(ProviderProfilesError::InvalidCredentialReference)
        );
        value.credential_env = Some("_SYNTHETIC_KEY_2".to_owned());
        assert_eq!(value.validate(), Ok(()));
        value.credential_env = None;
    }
    for invalid in [
        "",
        "2KEY",
        "KEY VALUE",
        "$KEY",
        "key-value",
        "KEY\n",
        "密钥",
    ] {
        value.credential_env = Some(invalid.to_owned());
        assert_eq!(
            value.validate(),
            Err(ProviderProfilesError::InvalidCredentialReference)
        );
    }
}

#[test]
fn ids_counts_and_orchestrator_references_are_validated() {
    let mut value = config();
    value.profiles.push(profile("primary"));
    assert_eq!(value.validate(), Err(ProviderProfilesError::DuplicateId));
    value.profiles.pop();
    value.orchestrator_profile_id = Some("missing".to_owned());
    assert_eq!(
        value.validate(),
        Err(ProviderProfilesError::UnknownSelection)
    );
    value.orchestrator_profile_id = None;
    for invalid in ["", "with space", "../path", "非ASCII", "newline\n"] {
        value.profiles[0].id = invalid.to_owned();
        assert_eq!(value.validate(), Err(ProviderProfilesError::InvalidId));
    }
    value.profiles = (0..MAX_PROVIDER_PROFILES)
        .map(|index| profile(&format!("profile-{index}")))
        .collect();
    assert_eq!(value.validate(), Ok(()));
    value.profiles.push(profile("extra"));
    assert_eq!(
        value.validate(),
        Err(ProviderProfilesError::TooManyProfiles)
    );
}

#[test]
fn field_limits_are_utf8_byte_limits() {
    let mut value = profile(&"i".repeat(64));
    value.name = "é".repeat(64);
    value.model = "m".repeat(256);
    value.credential_env = Some("K".repeat(128));
    value.base_url = format!("https://example.test/{}", "p".repeat(2027));
    assert_eq!(value.base_url.len(), 2048);
    assert_eq!(value.validate(), Ok(()));
    value.id.push('i');
    assert_eq!(value.validate(), Err(ProviderProfilesError::InvalidId));
    value.id.pop();
    value.name.push('é');
    assert_eq!(value.validate(), Err(ProviderProfilesError::InvalidName));
    value.name.pop();
    value.model.push('m');
    assert_eq!(value.validate(), Err(ProviderProfilesError::InvalidModel));
    value.model.pop();
    value.base_url.push('p');
    assert_eq!(value.validate(), Err(ProviderProfilesError::InvalidBaseUrl));
    value.base_url.pop();
    value.credential_env.as_mut().expect("reference").push('K');
    assert_eq!(
        value.validate(),
        Err(ProviderProfilesError::InvalidCredentialReference)
    );
}

#[test]
fn display_and_model_fields_refuse_padding_and_controls() {
    for text in ["", " padded", "padded ", "a\nb", "a\0b", "a\u{7f}b"] {
        let mut value = profile("primary");
        value.name = text.to_owned();
        assert_eq!(value.validate(), Err(ProviderProfilesError::InvalidName));
        value.name = "Valid".to_owned();
        value.model = text.to_owned();
        assert_eq!(value.validate(), Err(ProviderProfilesError::InvalidModel));
    }
}

#[test]
fn command_decoding_rejects_unknown_fields_and_protocols_without_echoing_values() {
    let original = serde_json::to_value(config()).expect("config");
    let mut invalid = original.clone();
    invalid["profiles"][0]["api_key"] = "do-not-echo-this-text".into();
    let error = ProviderProfilesConfig::from_command_value(invalid).expect_err("unknown field");
    assert_eq!(error, ProviderProfilesError::InvalidDocument);
    let command_error = serde_json::to_string(&CommandError::from(error)).expect("error");
    assert!(!command_error.contains("do-not-echo-this-text"));
    let mut invalid = original.clone();
    invalid["profiles"][0]["api_type"] = "openai_responses".into();
    assert_eq!(
        ProviderProfilesConfig::from_command_value(invalid),
        Err(ProviderProfilesError::InvalidDocument)
    );
    let mut invalid = original;
    invalid["readiness"] = "live".into();
    assert_eq!(
        ProviderProfilesConfig::from_command_value(invalid),
        Err(ProviderProfilesError::InvalidDocument)
    );
}

#[test]
fn oversized_command_input_stops_at_the_application_byte_limit() {
    let value = serde_json::json!({"unrecognized": "x".repeat(MAX_PROVIDER_PROFILES_BYTES)});
    assert_eq!(
        ProviderProfilesConfig::from_command_value(value),
        Err(ProviderProfilesError::TooLarge)
    );
    let mut writer = LimitedJson(Vec::new());
    writer
        .write_all(&vec![b'x'; MAX_PROVIDER_PROFILES_BYTES])
        .expect("exact bound");
    assert!(writer.write_all(b"x").is_err());
    assert_eq!(writer.0.len(), MAX_PROVIDER_PROFILES_BYTES);
}

#[test]
fn invalid_files_are_preserved_and_cannot_be_silently_replaced() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    let valid = config();
    let mut future = valid.clone();
    future.schema_version += 1;
    let cases = [
        (b"{invalid".to_vec(), ProviderProfilesError::InvalidDocument),
        (
            serde_json::to_vec(&future).expect("future schema"),
            ProviderProfilesError::UnsupportedSchema,
        ),
        (
            vec![b' '; MAX_PROVIDER_PROFILES_BYTES + 1],
            ProviderProfilesError::TooLarge,
        ),
    ];
    for (bytes, expected) in cases {
        fs::write(&path, &bytes).expect("fixture");
        assert_eq!(load_provider_profiles_at(&path), Err(expected));
        assert_eq!(save_provider_profiles_at(&path, &valid), Err(expected));
        assert_eq!(fs::read(&path).expect("preserved"), bytes);
        assert_eq!(fs::read_dir(directory.path()).expect("entries").count(), 1);
    }
}

#[test]
fn a_file_at_the_exact_byte_limit_is_accepted_but_one_more_byte_is_not() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    let value = config();
    let mut bytes = serde_json::to_vec(&value).expect("config");
    bytes.resize(MAX_PROVIDER_PROFILES_BYTES, b' ');
    fs::write(&path, &bytes).expect("file");
    assert_eq!(load_provider_profiles_at(&path), Ok(value));
    bytes.push(b' ');
    fs::write(&path, &bytes).expect("oversized");
    assert_eq!(
        load_provider_profiles_at(&path),
        Err(ProviderProfilesError::TooLarge)
    );
}

#[test]
fn invalid_updates_leave_the_last_valid_configuration_unchanged() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    let original = config();
    save_provider_profiles_at(&path, &original).expect("initial save");
    let bytes = fs::read(&path).expect("original bytes");
    let mut invalid = original.clone();
    invalid.profiles[0].base_url = "http://remote.example.test".to_owned();
    assert_eq!(
        save_provider_profiles_at(&path, &invalid),
        Err(ProviderProfilesError::InvalidBaseUrl)
    );
    assert_eq!(load_provider_profiles_at(&path), Ok(original));
    assert_eq!(fs::read(&path).expect("preserved"), bytes);
    assert_eq!(fs::read_dir(directory.path()).expect("entries").count(), 1);
}

#[test]
fn a_directory_is_never_opened_or_replaced_as_configuration() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    fs::create_dir(&path).expect("directory fixture");
    assert_eq!(
        load_provider_profiles_at(&path),
        Err(ProviderProfilesError::UnsafeFile)
    );
    assert_eq!(
        save_provider_profiles_at(&path, &config()),
        Err(ProviderProfilesError::UnsafeFile)
    );
    assert!(path.is_dir());
}

#[cfg(unix)]
#[test]
fn symlinks_and_fifos_are_refused_without_affecting_their_targets() {
    let directory = tempfile::tempdir().expect("directory");
    let target = directory.path().join("unrelated.json");
    fs::write(&target, b"unrelated user data").expect("target");
    let link = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    std::os::unix::fs::symlink(&target, &link).expect("link");
    assert_eq!(
        load_provider_profiles_at(&link),
        Err(ProviderProfilesError::UnsafeFile)
    );
    assert_eq!(
        save_provider_profiles_at(&link, &config()),
        Err(ProviderProfilesError::UnsafeFile)
    );
    assert_eq!(
        fs::read(&target).expect("preserved"),
        b"unrelated user data"
    );
    assert_eq!(fs::read_link(&link).expect("preserved link"), target);
    let fifo = directory.path().join("pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success()
    );
    assert_eq!(
        load_provider_profiles_at(&fifo),
        Err(ProviderProfilesError::UnsafeFile)
    );
}

#[cfg(unix)]
#[test]
fn newly_published_profiles_are_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join(PROVIDER_PROFILES_FILE_NAME);
    save_provider_profiles_at(&path, &config()).expect("save");
    assert_eq!(
        fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn debug_output_excludes_operator_metadata() {
    let value = config();
    let text = format!("{value:?}");
    for private in [
        "Synthetic provider",
        "provider.example.test",
        "SYNTHETIC_PROVIDER_KEY",
        "operator-model-alias",
    ] {
        assert!(!text.contains(private));
    }
    assert!(text.contains("profile_count: 1"));
}

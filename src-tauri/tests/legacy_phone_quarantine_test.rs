use tauri_app_lib::release_profile;

const APP_SETUP: &str = include_str!("../src/lib.rs");
const FRONTEND_PROFILE: &str = include_str!("../../src/releaseProfile.ts");
const FRONTEND_API: &str = include_str!("../../src/api.ts");

#[test]
fn legacy_phone_lan_is_disabled_for_every_release_profile() {
    assert!(
        !release_profile::phone_lan(),
        "the broad LAN/PWA bridge must stay disabled while the native companion is built"
    );
}

#[test]
fn application_startup_cannot_bind_the_legacy_phone_server() {
    for forbidden in [
        "phone::load_or_make_token(",
        "phone::bind_https(",
        "phone::serve(",
        "phone access ready (full app)",
    ] {
        assert!(
            !APP_SETUP.contains(forbidden),
            "application setup reintroduced legacy phone startup: {forbidden}"
        );
    }
}

#[test]
fn frontend_profiles_and_transport_cannot_revive_the_browser_bridge() {
    assert!(
        FRONTEND_PROFILE.contains("phoneLan: false"),
        "the frontend release profile must keep the legacy phone UI disabled"
    );
    for forbidden in [
        "noted_token",
        "fetch(`/api/",
        "new URLSearchParams(window.location.search)",
    ] {
        assert!(
            !FRONTEND_API.contains(forbidden),
            "the frontend reintroduced legacy URL-token transport: {forbidden}"
        );
    }
}

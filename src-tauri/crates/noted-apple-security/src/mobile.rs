use crate::models::*;
use crate::record_crypto::{
    OpenRecordArgs, OpenedRecordBridge, OpenedRecordV1, RecordCiphertextBridge, RecordCiphertextV1,
    RecordCryptoContextV1, SealRecordArgs,
};
use crate::{Error, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::plugin::{PluginApi, PluginHandle};
use tauri::{AppHandle, Runtime};

tauri::ios_plugin_binding!(init_plugin_noted_apple_security);

pub(crate) fn init<R: Runtime>(
    _app: &AppHandle<R>,
    api: PluginApi<R, ()>,
) -> Result<AppleSecurity<R>> {
    let handle = api.register_ios_plugin(init_plugin_noted_apple_security)?;
    Ok(AppleSecurity(handle))
}

pub struct AppleSecurity<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> AppleSecurity<R> {
    pub fn prepare_identity(&self, device_id: &str) -> Result<PublicIdentity> {
        self.prepare_identity_with_gate(device_id, None)
    }

    #[cfg(feature = "sanitized-development-fixtures")]
    pub fn prepare_sanitized_development_fixture_identity(
        &self,
        device_id: &str,
    ) -> Result<PublicIdentity> {
        self.prepare_identity_with_gate(device_id, Some(fixture_gate()))
    }

    fn prepare_identity_with_gate(
        &self,
        device_id: &str,
        fixture_gate: Option<&str>,
    ) -> Result<PublicIdentity> {
        crate::models::validate_uuid_v7(device_id)?;
        let wire: PublicIdentityWire = self.0.run_mobile_plugin(
            "prepareIdentity",
            PrepareIdentityArgs {
                device_id,
                fixture_gate,
            },
        )?;
        wire.try_into()
    }

    pub fn identity(&self, handle: &IdentityHandle) -> Result<PublicIdentity> {
        let wire: PublicIdentityWire = self.0.run_mobile_plugin(
            "getIdentity",
            IdentityArgs {
                handle: handle.expose_opaque(),
            },
        )?;
        wire.try_into()
    }

    /// Public-only inventory used to reconcile Keychain state after a crash or
    /// reinstall without persisting private key material in the app database.
    pub fn identity_inventory(&self) -> Result<IdentityInventory> {
        let wire: IdentityInventoryWire = self.0.run_mobile_plugin("listIdentities", ())?;
        wire.try_into()
    }

    pub fn sign(&self, handle: &IdentityHandle, message: &[u8]) -> Result<Vec<u8>> {
        let wire: SignatureWire = self
            .0
            .run_mobile_plugin("sign", SignArgs::new(handle, message)?)?;
        wire.decode()
    }

    pub fn verify_p256_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<bool> {
        let wire: VerificationWire = self.0.run_mobile_plugin(
            "verifyP256Signature",
            VerifySignatureArgs::new(public_key, message, signature)?,
        )?;
        Ok(wire.valid)
    }

    /// Seal one canonical fixture record under the active native-only library
    /// key. The returned descriptor contains ciphertext and public bindings;
    /// the library key never crosses the plugin boundary.
    pub fn seal_record(
        &self,
        identity: &IdentityHandle,
        context: &RecordCryptoContextV1,
        plaintext: &[u8],
    ) -> Result<RecordCiphertextV1> {
        let wire: RecordCiphertextBridge = self.0.run_mobile_plugin(
            "sealRecord",
            SealRecordArgs::new(identity, context, plaintext)?,
        )?;
        wire.into_public(context)
    }

    /// Verify the inner record-envelope signature and open it with the
    /// active library key. `signer_public_key` is the expected enrolled P-256
    /// X9.63 key selected by the Rust sync authority, never a key from the
    /// untrusted ciphertext descriptor. This signature authenticates the
    /// encrypted record only; callers must independently create or verify the
    /// outer mutation-envelope signature over `MutationEnvelope::signing_bytes`.
    /// Bootstrap and pull must remain closed until the caller has an
    /// authority-authenticated historical writer-key directory.
    pub fn open_record(
        &self,
        identity: &IdentityHandle,
        context: &RecordCryptoContextV1,
        sealed: &RecordCiphertextV1,
        signer_public_key: &[u8],
    ) -> Result<OpenedRecordV1> {
        let wire: OpenedRecordBridge = self.0.run_mobile_plugin(
            "openRecord",
            OpenRecordArgs::new(identity, context, sealed, signer_public_key)?,
        )?;
        wire.into_public(context, sealed)
    }

    pub fn fresh_bytes(&self, length: usize) -> Result<Vec<u8>> {
        if !(1..=64).contains(&length) {
            return Err(Error::InvalidNativeResponse("secure random byte count"));
        }
        let wire: FreshBytesWire = self
            .0
            .run_mobile_plugin("freshBytes", FreshBytesArgs { length })?;
        wire.decode(length)
    }

    pub fn fresh_uuid_v7(&self) -> Result<String> {
        let wire: FreshUuidV7Wire = self.0.run_mobile_plugin("freshUUIDv7", ())?;
        crate::models::validate_uuid_v7(&wire.value)?;
        Ok(wire.value)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_authenticated_hpke(
        &self,
        handle: &IdentityHandle,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        encapsulated_key: &[u8],
        ciphertext: &[u8],
        exporter_context: &[u8],
    ) -> Result<OpenedHpke> {
        let wire: OpenHpkeWire = self.0.run_mobile_plugin(
            "openAuthenticatedHpke",
            OpenHpkeArgs::new(
                handle,
                sender_public_key,
                info,
                associated_data,
                encapsulated_key,
                ciphertext,
                exporter_context,
            )?,
        )?;
        wire.try_into()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stage_bootstrap_authenticated(
        &self,
        handle: &IdentityHandle,
        sender_public_key: &[u8],
        info: &[u8],
        associated_data: &[u8],
        encapsulated_key: &[u8],
        ciphertext: &[u8],
        receipt_id: &str,
        envelope_digest: &[u8],
        metadata: &BootstrapMetadataV1,
    ) -> Result<StagedBootstrapDescriptor> {
        let wire: StageBootstrapWire = self.0.run_mobile_plugin(
            "stageBootstrapAuthenticated",
            StageBootstrapArgs::new(
                handle,
                sender_public_key,
                info,
                associated_data,
                encapsulated_key,
                ciphertext,
                receipt_id,
                envelope_digest,
                metadata,
            )?,
        )?;
        wire.metadata.validate()?;
        if &wire.metadata != metadata {
            return Err(Error::InvalidNativeResponse(
                "native bootstrap metadata mismatch",
            ));
        }
        Ok(StagedBootstrapDescriptor {
            pending_bootstrap_handle: PendingBootstrapHandle::parse(wire.pending_bootstrap_handle)?,
            metadata: wire.metadata,
        })
    }

    pub fn activate_bootstrap(
        &self,
        identity: &IdentityHandle,
        pending: &PendingBootstrapHandle,
        receipt_id: &str,
    ) -> Result<PublicIdentity> {
        let wire: PublicIdentityWire = self.0.run_mobile_plugin(
            "activateBootstrap",
            BootstrapTransitionArgs {
                identity_handle: identity.expose_opaque(),
                pending_bootstrap_handle: pending.expose_opaque(),
                receipt_id,
            },
        )?;
        wire.try_into()
    }

    pub fn discard_pending(
        &self,
        identity: &IdentityHandle,
        pending: Option<&PendingBootstrapHandle>,
        receipt_id: Option<&str>,
    ) -> Result<PublicIdentity> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Args<'a> {
            identity_handle: &'a str,
            pending_bootstrap_handle: Option<&'a str>,
            receipt_id: Option<&'a str>,
        }
        let wire: PublicIdentityWire = self.0.run_mobile_plugin(
            "discardPending",
            Args {
                identity_handle: identity.expose_opaque(),
                pending_bootstrap_handle: pending.map(PendingBootstrapHandle::expose_opaque),
                receipt_id,
            },
        )?;
        wire.try_into()
    }

    pub fn protected_data_state(&self) -> Result<ProtectedDataState> {
        #[derive(serde::Deserialize)]
        struct Response {
            state: ProtectedDataState,
        }
        self.0
            .run_mobile_plugin::<Response>("protectedDataState", ())
            .map(|response| response.state)
            .map_err(Error::from)
    }

    pub fn subscribe_protected_data<F>(&self, callback: F) -> Result<()>
    where
        F: Fn(ProtectedDataEvent) + Send + Sync + 'static,
    {
        #[derive(Serialize)]
        struct Args {
            handler: Channel,
        }
        let handler = Channel::new(move |event| {
            if let InvokeResponseBody::Json(payload) = event {
                if let Ok(event) = serde_json::from_str::<ProtectedDataEvent>(&payload) {
                    callback(event);
                }
            }
            Ok(())
        });
        self.0
            .run_mobile_plugin::<()>("subscribeProtectedData", Args { handler })
            .map_err(Error::from)
    }

    pub fn harden_store_files(
        &self,
        database_path: &Path,
        recovery_paths: &[PathBuf],
    ) -> Result<StoreProtectionReport> {
        self.0
            .run_mobile_plugin(
                "hardenStoreFiles",
                HardenStoreArgs::new(database_path, recovery_paths)?,
            )
            .map_err(Error::from)
    }

    /// Protect the dedicated store directory before SQLite creates the database
    /// or sidecars. A post-open `harden_store_files` call is still mandatory.
    pub fn prepare_store_directory(
        &self,
        database_path: &Path,
        recovery_paths: &[PathBuf],
    ) -> Result<StoreProtectionReport> {
        self.0
            .run_mobile_plugin(
                "prepareStoreDirectory",
                HardenStoreArgs::new(database_path, recovery_paths)?,
            )
            .map_err(Error::from)
    }

    pub fn verify_store_files(
        &self,
        database_path: &Path,
        recovery_paths: &[PathBuf],
    ) -> Result<StoreProtectionReport> {
        self.0
            .run_mobile_plugin(
                "verifyStoreFiles",
                HardenStoreArgs::new(database_path, recovery_paths)?,
            )
            .map_err(Error::from)
    }
}

import { useEffect, useMemo, useState } from "react";
import { Check, Copy, Laptop, ShieldCheck, Smartphone, X } from "lucide-react";
import QRCode from "qrcode";
import { api, type MobileAuthorityInfo } from "./api";

export function PhonePanel({ onClose }: { onClose: () => void }) {
  const [info, setInfo] = useState<MobileAuthorityInfo | null>(null);
  const [qr, setQr] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const pairingCode = useMemo(() => info ? JSON.stringify({
    invitationJson: info.invitationJson,
    address: `${info.address}:${info.port}`,
  }) : "", [info]);
  const secondsRemaining = info
    ? Math.max(0, Math.ceil((info.invitationExpiresAtMs - Date.now()) / 1_000))
    : 0;

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    const refresh = () => api.mobileAuthorityStart()
      .then((next) => {
        if (!cancelled) {
          setInfo(next);
          setError(null);
        }
      })
      .catch((reason) => { if (!cancelled) setError(String(reason)); });
    void refresh();
    timer = window.setInterval(() => {
      void refresh();
    }, 1_000);
    return () => { cancelled = true; if (timer) window.clearInterval(timer); };
  }, []);

  useEffect(() => {
    if (!pairingCode) return;
    void QRCode.toDataURL(pairingCode, {
      margin: 2,
      width: 280,
      errorCorrectionLevel: "L",
      color: { dark: "#070707", light: "#ffffff" },
    }).then(setQr).catch((reason) => setError(String(reason)));
  }, [pairingCode]);

  async function copyCode() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      // Copying is the start of a pairing attempt. Renew here so the pasted
      // code always receives the protocol's complete five-minute lifetime.
      const fresh = await api.mobileAuthorityStart(true);
      const freshCode = JSON.stringify({
        invitationJson: fresh.invitationJson,
        address: `${fresh.address}:${fresh.port}`,
      });
      setInfo(fresh);
      await navigator.clipboard.writeText(freshCode);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_500);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function confirm(approved: boolean) {
    const pending = info?.pendingConfirmation;
    if (!pending || busy) return;
    setBusy(true);
    setError(null);
    try {
      setInfo(await api.mobileAuthorityConfirm(pending.receiptId, pending.verificationCode, approved));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal phone-pairing-modal" onClick={(event) => event.stopPropagation()}>
        <button className="modal-close" onClick={onClose} aria-label="Close"><X /></button>
        <div className="phone-pairing-modal__title"><span><Laptop /></span><div><p>IPHONE COMPANION</p><h3>Connect your iPhone</h3></div></div>
        {error && <div className="error">{error}</div>}
        {info?.pendingConfirmation ? (
          <section className="phone-confirmation">
            <ShieldCheck />
            <p>Make sure this code matches the one on your iPhone.</p>
            <strong>{info.pendingConfirmation.verificationCode}</strong>
            <small>Access: test notes, folders, and categories</small>
            <div><button type="button" onClick={() => void confirm(false)} disabled={busy}>Deny</button><button type="button" className="primary" onClick={() => void confirm(true)} disabled={busy}><Check /> Approve iPhone</button></div>
          </section>
        ) : info ? (
          <>
            <p className="muted">Keep Noted open on both devices and use the same Wi-Fi. This development preview pairs securely with an isolated test library; it does not read your personal Mac library yet.</p>
            <div className="phone-pairing-code">
              {qr && <img className="qr" src={qr} alt="Noted iPhone pairing code" />}
              <div><Smartphone /><strong>On your iPhone</strong><span>Tap Connect Mac, then paste the pairing code. The code is one-time and expires in {Math.floor(secondsRemaining / 60)}:{String(secondsRemaining % 60).padStart(2, "0")}.</span><button type="button" onClick={() => void copyCode()} disabled={busy}>{copied ? <Check /> : <Copy />}{copied ? "Fresh code copied" : busy ? "Generating…" : "Copy fresh code"}</button></div>
            </div>
            <code className="phone-url">{info.address}:{info.port}</code>
          </>
        ) : (
          <p className="muted">Starting private sync…</p>
        )}
      </div>
    </div>
  );
}

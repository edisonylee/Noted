import { useEffect, useState } from "react";
import QRCode from "qrcode";
import { api, type PhoneInfo } from "./api";

export function PhonePanel({ onClose }: { onClose: () => void }) {
  const [info, setInfo] = useState<PhoneInfo | null>(null);
  const [qr, setQr] = useState("");
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    api
      .phoneInfo()
      .then(async (i) => {
        setInfo(i);
        if (i.url) {
          setQr(
            await QRCode.toDataURL(i.url, {
              margin: 1,
              width: 240,
              color: { dark: "#0c0a09", light: "#ffffff" },
            })
          );
        }
      })
      .catch((e) => setErr(String(e)));
  }, []);

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <button className="modal-close" onClick={onClose}>
          ✕
        </button>
        <h3>Use noted on your phone</h3>
        {err && <div className="error">{err}</div>}
        {info && info.url ? (
          <>
            <p className="muted">
              On the same wifi, scan with your phone’s camera to open the full app. You only do this
              once — then <strong>Share → Add to Home Screen</strong> installs noted as an app icon
              you can tap any time. The link stays the same across restarts.
            </p>
            {qr && <img className="qr" src={qr} alt="scan to open noted on your phone" />}
            <code className="phone-url">{info.url}</code>
            {info.lan_url && info.lan_url !== info.url && (
              <p className="muted phone-fallback">
                Can’t connect? Use this address instead: <code>{info.lan_url}</code>
              </p>
            )}
            <p className="muted phone-cert-note">
              Your phone will warn that the connection isn’t private (it’s a self-signed
              certificate on your own machine) — tap through to continue. One time only.
            </p>
          </>
        ) : info ? (
          <p className="muted">Couldn’t start the phone server (port busy). Restart noted to retry.</p>
        ) : (
          <p className="muted">starting phone server…</p>
        )}
      </div>
    </div>
  );
}

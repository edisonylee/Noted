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
        <h3>Capture from your phone</h3>
        {err && <div className="error">{err}</div>}
        {info && info.url ? (
          <>
            <p className="muted">
              Scan with your phone’s camera on the same wifi, snap a note, and it shows up here for review.
            </p>
            {qr && <img className="qr" src={qr} alt="scan to capture" />}
            <code className="phone-url">{info.url}</code>
          </>
        ) : info ? (
          <p className="muted">Couldn’t start the capture server (port busy). Restart noted to retry.</p>
        ) : (
          <p className="muted">starting capture server…</p>
        )}
      </div>
    </div>
  );
}

export type Img = { base64: string; ext: string; dataUrl: string };

// Read a File into base64 + a data URL for preview. Shared by the desktop
// capture flow and the mobile capture screen.
//
// Apple's HEIC/HEIF photos are transcoded to JPEG up front: the webview can
// decode them but our vision providers and most non-Apple viewers can't, so we
// normalise once here and everything downstream (preview, storage, OCR) sees a
// plain JPEG.
export async function fileToImg(file: File): Promise<Img> {
  if (isHeic(file)) {
    try {
      return await heicToJpeg(file);
    } catch {
      // Fall through to the raw read; better to keep the original bytes than
      // to drop the photo entirely if the webview can't decode HEIC.
    }
  }
  const dataUrl = await readDataUrl(file);
  return fromDataUrl(dataUrl);
}

function isHeic(file: File): boolean {
  const t = file.type.toLowerCase();
  if (t === "image/heic" || t === "image/heif") return true;
  // Some platforms hand us an empty type, so fall back to the extension.
  return /\.(heic|heif)$/i.test(file.name);
}

function readDataUrl(file: Blob): Promise<string> {
  return new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(r.result as string);
    r.onerror = rej;
    r.readAsDataURL(file);
  });
}

function fromDataUrl(dataUrl: string): Img {
  const base64 = dataUrl.split(",")[1] ?? "";
  const mime = dataUrl.slice(5, dataUrl.indexOf(";"));
  const ext = mime.includes("jpeg") ? "jpg" : mime.split("/")[1] || "png";
  return { base64, ext, dataUrl };
}

// Decode the HEIC via the webview's native image support, paint it onto a
// canvas, and re-encode as JPEG.
async function heicToJpeg(file: File): Promise<Img> {
  const url = URL.createObjectURL(file);
  try {
    const bitmap = await loadImage(url);
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.naturalWidth;
    canvas.height = bitmap.naturalHeight;
    const ctx = canvas.getContext("2d");
    if (!ctx || !canvas.width || !canvas.height) {
      throw new Error("heic decode failed");
    }
    ctx.drawImage(bitmap, 0, 0);
    const dataUrl = canvas.toDataURL("image/jpeg", 0.92);
    return fromDataUrl(dataUrl);
  } finally {
    URL.revokeObjectURL(url);
  }
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((res, rej) => {
    const img = new Image();
    img.onload = () => res(img);
    img.onerror = () => rej(new Error("image load failed"));
    img.src = src;
  });
}

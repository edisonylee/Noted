export type Img = { base64: string; ext: string; dataUrl: string };

// Read a File into base64 + a data URL for preview. Shared by the desktop
// capture flow and the mobile capture screen.
export async function fileToImg(file: File): Promise<Img> {
  const dataUrl: string = await new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(r.result as string);
    r.onerror = rej;
    r.readAsDataURL(file);
  });
  const base64 = dataUrl.split(",")[1] ?? "";
  const mime = dataUrl.slice(5, dataUrl.indexOf(";"));
  const ext = mime.includes("jpeg") ? "jpg" : mime.split("/")[1] || "png";
  return { base64, ext, dataUrl };
}

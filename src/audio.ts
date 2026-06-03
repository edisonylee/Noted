// Microphone capture -> 16 kHz mono f32 PCM (base64), the format the Rust
// `transcribe` command (whisper.cpp) expects. We record with MediaRecorder,
// decode with Web Audio (handles whatever codec the platform records), then
// downsample to 16 kHz ourselves.

export type Recorder = { stop: () => Promise<{ b64: string; sampleRate: number }>; cancel: () => void };

export async function startRecording(): Promise<Recorder> {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const rec = new MediaRecorder(stream);
  const chunks: BlobPart[] = [];
  rec.ondataavailable = (e) => {
    if (e.data.size) chunks.push(e.data);
  };
  rec.start();

  const cleanup = () => stream.getTracks().forEach((t) => t.stop());

  return {
    cancel: () => {
      try {
        rec.stop();
      } catch {
        /* ignore */
      }
      cleanup();
    },
    stop: () =>
      new Promise((resolve, reject) => {
        rec.onstop = async () => {
          try {
            const blob = new Blob(chunks, { type: rec.mimeType || "audio/webm" });
            const buf = await blob.arrayBuffer();
            const ctx = new AudioContext();
            const audio = await ctx.decodeAudioData(buf);
            const mono = audio.getChannelData(0);
            const down = downsample(mono, audio.sampleRate, 16000);
            await ctx.close();
            cleanup();
            resolve({ b64: floatToBase64(down), sampleRate: 16000 });
          } catch (e) {
            cleanup();
            reject(e);
          }
        };
        rec.stop();
      }),
  };
}

function downsample(input: Float32Array, from: number, to: number): Float32Array {
  if (from === to) return input;
  const ratio = to / from;
  const outLen = Math.floor(input.length * ratio);
  const out = new Float32Array(outLen);
  for (let i = 0; i < outLen; i++) {
    const src = i / ratio;
    const j = Math.floor(src);
    const frac = src - j;
    const a = input[j] ?? 0;
    const b = input[j + 1] ?? a;
    out[i] = a + (b - a) * frac;
  }
  return out;
}

function floatToBase64(f: Float32Array): string {
  const bytes = new Uint8Array(f.buffer, f.byteOffset, f.byteLength);
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode.apply(null, Array.from(bytes.subarray(i, i + CHUNK)));
  }
  return btoa(bin);
}

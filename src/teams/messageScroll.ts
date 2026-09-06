export type MessagePosition = { id: string; seq: number; offset: number };
const positions = new Map<string, MessagePosition>();
export const readMessagePosition = (key: string) => positions.get(key);
export function saveMessagePosition(key: string, position: MessagePosition) {
  positions.set(key, position);
}
export function captureMessagePosition(viewport: HTMLElement): MessagePosition | undefined {
  const top = viewport.getBoundingClientRect().top;
  const row = [...viewport.querySelectorAll<HTMLElement>("[data-message-id]")]
    .find((element) => element.getBoundingClientRect().bottom > top);
  if (!row) return;
  return { id: row.dataset.messageId!, seq: Number(row.dataset.messageSeq), offset: row.getBoundingClientRect().top - top };
}
export function restoreMessagePosition(viewport: HTMLElement, position: MessagePosition) {
  const row = [...viewport.querySelectorAll<HTMLElement>("[data-message-id]")]
    .find((element) => element.dataset.messageId === position.id);
  if (!row) return false;
  viewport.scrollTop += row.getBoundingClientRect().top - viewport.getBoundingClientRect().top - position.offset;
  return true;
}

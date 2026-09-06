import { useBackdropDismiss } from "./ui/useDismissal";
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ArrowUpRight, FileText, Mic, Search, X, type LucideIcon } from 'lucide-react';
import type { NoteRow } from './api';
import './FloatingDock.css';

export type DockDestination = { id: string; label: string; icon: LucideIcon; active: boolean; onSelect: () => void };

export function FloatingDock({ open, onOpenChange, destinations, notes, onOpenNote, recording, onRecording, children }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  destinations: DockDestination[];
  notes: NoteRow[];
  onOpenNote: (note: NoteRow) => void;
  recording: boolean;
  onRecording: () => void;
  children: ReactNode;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const [query, setQuery] = useState('');
  const needle = query.trim().toLowerCase();
  const matches = destinations.filter(item => item.label.toLowerCase().includes(needle));
  const recent = notes.filter(note => !note.trashed_at && (!needle || `${note.title} ${note.raw_text}`.toLowerCase().includes(needle)))
    .sort((a, b) => (b.updated_at || b.created_at).localeCompare(a.updated_at || a.created_at)).slice(0, 8);

  const backdrop = useBackdropDismiss(() => onOpenChange(false), false, true);
  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    if (open && !element.open) {
      returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : trigger.current;
      setQuery('');
      element.showModal();
      search.current?.focus();
    } else if (!open && element.open) {
      element.close();
      (returnFocus.current?.isConnected ? returnFocus.current : trigger.current)?.focus();
    }
  }, [open]);

  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      const editable = event.target instanceof HTMLElement && !!event.target.closest('input, textarea, [contenteditable="true"]');
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k' && (event.shiftKey || !editable)) {
        if (document.querySelector('dialog[open]') && !dialog.current?.open) return;
        event.preventDefault();
        onOpenChange(!open);
      }
    };
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [open, onOpenChange]);

  function select(action: () => void) {
    action();
    onOpenChange(false);
  }

  return <>
    <nav className="floating-dock" aria-label="Main navigation">
      <button ref={trigger} className="dock-brand" aria-label="Open navigation and search" title="Navigation and search (⌘K)" aria-haspopup="dialog" aria-expanded={open} onClick={() => onOpenChange(!open)}>
        <img src="/noted-logo.png" alt="" draggable={false} />
      </button>
      <span className="dock-divider" />
      {destinations.map(({ id, label, icon: Icon, active, onSelect }) => <button key={id} className="dock-destination" aria-label={label} aria-current={active ? 'page' : undefined} onClick={onSelect}>
        <Icon size={20} strokeWidth={1.6} /><span className="dock-tooltip" aria-hidden>{label}</span>
      </button>)}
      <span className="dock-divider" />
      <button className={`dock-destination dock-record${recording ? ' is-recording' : ''}`} aria-label={recording ? 'Open active recording' : 'Recording options'} onClick={onRecording}>
        <Mic size={20} strokeWidth={1.6} /><span className="dock-tooltip" aria-hidden>{recording ? 'Recording in progress' : 'Record a meeting'}</span>
      </button>
    </nav>
    <dialog ref={dialog} className="mission-dialog" aria-labelledby="mission-title" onCancel={event => { event.preventDefault(); onOpenChange(false); }} {...backdrop}>
      <div className="mission-surface">
        <header className="mission-heading"><div><h2 id="mission-title">Your space</h2><p>Find a thought. Pick up where you left off.</p></div><button aria-label="Close navigation" onClick={() => onOpenChange(false)}><X size={18} /></button></header>
        <label className="mission-search"><Search size={18} /><input ref={search} aria-label="Search destinations, documents, and captures" placeholder="Search your space…" value={query} onChange={event => setQuery(event.target.value)} onKeyDown={event => {
          if (event.key === 'ArrowDown') { event.preventDefault(); dialog.current?.querySelector<HTMLButtonElement>('.mission-result')?.focus(); }
          if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
            event.preventDefault();
            if (needle && matches[0]) select(matches[0].onSelect);
            else if (recent[0]) select(() => onOpenNote(recent[0]));
          }
        }} /><kbd>esc</kbd></label>
        <div className="mission-body">
          <div className="mission-controls" onClick={event => { if (event.target instanceof Element && event.target.closest('.side-nav button, .mission-settings')) onOpenChange(false); }}>{children}</div>
          <section className="mission-results" aria-label="Search results" onKeyDown={event => {
            if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
            const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('.mission-result')];
            const index = buttons.indexOf(event.target as HTMLButtonElement);
            if (index < 0) return;
            event.preventDefault();
            if (event.key === 'ArrowUp' && index === 0) search.current?.focus();
            else buttons[Math.min(buttons.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1))]?.focus();
          }}>
            {needle && matches.length > 0 && <><h3>Go to</h3>{matches.map(item => <button className="mission-result" key={item.id} onClick={() => select(item.onSelect)}><item.icon size={17} /><span>{item.label}</span><ArrowUpRight size={14} /></button>)}</>}
            <h3>{needle ? 'Matching items' : 'Recent items'}</h3>
            {recent.length ? recent.map(note => <button className="mission-result" key={note.id} onClick={() => select(() => onOpenNote(note))}><FileText size={17} /><span><strong>{note.title.trim() || note.raw_text.trim().split('\n')[0] || 'Untitled note'}</strong><small>{note.note_kind === 'document' ? 'Document' : 'Capture'} · {note.raw_text.replace(/\s+/g, ' ').slice(0, 100) || 'Open note'}</small></span><ArrowUpRight size={14} /></button>) : <p className="mission-empty">{needle ? 'No matching documents or captures. Try another word.' : 'Your recent documents and captures will appear here.'}</p>}
          </section>
        </div>
        <footer className="mission-footer"><span>⌘K to navigate · ⌘⇧K while editing</span><span>esc to close</span></footer>
      </div>
    </dialog>
  </>;
}

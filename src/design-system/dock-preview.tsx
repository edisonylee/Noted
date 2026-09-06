import { useState, type CSSProperties } from 'react';
import { createRoot } from 'react-dom/client';
import { Mic, Moon, Settings, Sun } from 'lucide-react';
import '@fontsource-variable/geist';
import '../App.css';
import './product.css';
import { FloatingDock } from '../FloatingDock';
import { PRIMARY_DESTINATIONS } from '../navigation';
import { isDocumentNote } from '../library';
import type { NoteRow } from '../api';
import { getNeonTokens, NEON_ACCENTS, type NeonAccent } from './tokens';

const samples: NoteRow[] = [
  { id: 1, title: 'Product conversation', raw_text: 'Start with one clear action. Let the workspace make room for the thought.', created_at: '2026-09-06T10:30:00', event_date: '2026-09-06', source: 'text', entries: [], document_json: null, note_kind: 'document', updated_at: '2026-09-06T10:30:00' },
  { id: 2, title: 'A thought from the walk', raw_text: 'Remembering is about finding the thread. Keep the thought and the context together.', created_at: '2026-09-06T08:42:00', event_date: '2026-09-06', source: 'text', entries: [], document_json: null, note_kind: 'capture', updated_at: '2026-09-06T08:42:00' },
  { id: 3, title: 'Design review', raw_text: 'The content should have the most presence. Everything else can step back.', created_at: '2026-09-05T14:00:00', event_date: '2026-09-05', source: 'text', entries: [], document_json: null, note_kind: 'capture', updated_at: '2026-09-06T08:42:00' },
];

function Preview() {
  const [open, setOpen] = useState(false);
  const [view, setView] = useState('documents');
  const [note, setNote] = useState(samples[0]);
  const [dark, setDark] = useState(false);
  const [accent, setAccent] = useState<NeonAccent>('citron');
  const [recording, setRecording] = useState(false);
  const t = getNeonTokens(accent, dark ? 'dark' : 'light');
  const style = { '--canvas': t.canvas, '--surface': t.raised, '--surface-2': t.surface, '--ink': t.ink, '--ink-soft': t.secondary, '--muted': t.muted, '--line': t.line, '--line-strong': t.controlLine, '--accent': t.accentInk, '--accent-fill': t.accent, '--accent-focus': t.focus, '--hover-soft': t.hover, '--bad': t.danger, background: t.canvas, color: t.ink, minHeight: '100vh', fontFamily: 'Geist Variable, sans-serif' } as CSSProperties;
  const destinations = PRIMARY_DESTINATIONS.map(page => ({ ...page, active: view === page.id, onSelect: () => setView(page.id) }));
  const viewLabel = PRIMARY_DESTINATIONS.find(page => page.id === view)?.label ?? "Settings";
  return <div style={style}>
    <FloatingDock open={open} onOpenChange={setOpen} destinations={destinations} notes={samples} onOpenNote={next => { setNote(next); setView(isDocumentNote(next) ? 'documents' : 'library'); }} recording={recording} onRecording={() => setOpen(true)}>
      <aside className="mission-utilities"><nav className="side-nav">{destinations.map(item => <button key={item.id} className={item.active ? 'on' : ''} onClick={item.onSelect}><item.icon size={16} />{item.label}</button>)}</nav><button className="rec-pill" onClick={() => setRecording(!recording)}><Mic size={16} />{recording ? 'Stop sample recording' : 'Start sample recording'}</button><div className="side-foot"><button className="icon-btn" aria-label="Toggle color mode" onClick={() => setDark(!dark)}>{dark ? <Sun size={18} /> : <Moon size={18} />}</button><button className="icon-btn mission-settings" aria-label="Settings" onClick={() => setView('Settings')}><Settings size={18} /></button></div></aside>
    </FloatingDock>
    <header style={{ height: 72, marginLeft: 96, padding: '0 28px', display: 'flex', alignItems: 'center', justifyContent: 'space-between', borderBottom: '1px solid var(--line)', gap: 16 }}><span style={{ fontSize: 13, color: t.muted }}>Noted / {viewLabel}</span><div style={{ display: 'flex', gap: 8 }}>{Object.entries(NEON_ACCENTS).map(([key, color]) => <button key={key} aria-label={color.name} aria-pressed={accent === key} onClick={() => setAccent(key as NeonAccent)} style={{ width: 28, height: 28, borderRadius: '50%', border: accent === key ? `2px solid ${t.ink}` : '2px solid transparent', background: color.vivid }} />)}</div></header>
    <main style={{ marginLeft: 96, padding: '56px clamp(24px, 6vw, 96px)', maxWidth: 1020 }}>
      {(view === 'documents' || view === 'library') ? <><p style={{ fontSize: 12, color: t.muted }}>Sample note · Today, 10:30 AM</p><h1 style={{ fontSize: 'clamp(30px, 4vw, 42px)', letterSpacing: '-.045em', margin: '14px 0 28px' }}>{note.title}</h1><p style={{ fontSize: 18, lineHeight: 1.75, maxWidth: 650 }}>{note.raw_text}</p><div style={{ margin: '44px 0', width: 44, height: 3, background: t.accent }} /><h2 style={{ fontSize: 18 }}>Space for the thought.</h2><p style={{ color: t.secondary, fontSize: 16, lineHeight: 1.8, maxWidth: 580 }}>Navigation stays within reach. Open the Noted icon to move between Documents, Library, Team, and your other workspaces.</p><label style={{ display: 'flex', gap: 10, marginTop: 32 }}><input type="checkbox" />Review the next capture experience</label></> : <><h1>{viewLabel}</h1><p style={{ marginTop: 24, color: t.secondary }}>Selected from the floating dock. This preview uses sample content.</p>{view === 'Settings' && <button className="pill" onClick={() => setDark(!dark)}>Switch to {dark ? 'light' : 'dark'} mode</button>}</>}
      <p style={{ marginTop: 80, fontSize: 12, color: t.muted }}>Navigation preview · sample notes · ⌘K to open</p>
    </main>
  </div>;
}
createRoot(document.getElementById('root')!).render(<Preview />);

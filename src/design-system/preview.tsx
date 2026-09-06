import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { ArrowLeft, ArrowUpRight, Check, ChevronRight, FileText, Layers, LockKeyhole, Mic, PanelRight, Plus, Search, X } from 'lucide-react';
import '@fontsource-variable/geist';
import { Button, CheckField, Citation, TextField } from './components';
import { DEFAULT_NEON_ACCENT, FOUNDATIONS, getNeonTokens, NEON_ACCENTS, neonCssVariables, type NeonAccent } from './tokens';
import type { ThemeMode } from '../themes/types';
import wordmark from './assets/wordmark.png';
import reference from './assets/brand-reference.png';
import './preview.css';

type Note = { id: string; title: string; kind: 'Meeting' | 'Note'; time: string; excerpt: string; summary: string; highlight: string; tasks: string[]; source: string[] };
const INITIAL_NOTES: Note[] = [
  { id: 'product', title: 'Product conversation', kind: 'Meeting', time: 'Today · 10:30 AM', excerpt: 'A simpler first-run experience.', summary: 'We walked through the first few minutes of Noted. The strongest direction was a quiet workspace that makes it easy to capture something, then return to it with its original context.', highlight: 'Start with one clear action.', tasks: ['Sketch the first capture experience', 'Review the source-to-note interaction'], source: ['We are asking people to make too many decisions before they have even saved a thought.', 'Start with one clear action. Let someone capture something and see where it goes.', 'When they come back, the original conversation should be right there beside the note.'] },
  { id: 'walk', title: 'A thought from the walk', kind: 'Note', time: 'Today · 8:42 AM', excerpt: 'Remembering is about finding the thread.', summary: 'The useful part of a note is often what surrounds it: the question that led to it, the person who said it, or the next thing it made you think of.', highlight: 'Keep the thought and the thread.', tasks: ['Try this idea in the next design review'], source: [] },
  { id: 'design', title: 'Design review', kind: 'Meeting', time: 'Yesterday · 2:00 PM', excerpt: 'Make room for the content.', summary: 'We compared the new reading surface with the existing layout. More space around the document made it easier to follow. Color worked best when it pointed to something meaningful.', highlight: 'The content should have the most presence.', tasks: ['Check the reading width on smaller screens', 'Test keyboard navigation'], source: ['There is a lot competing with the note right now.', 'The content should have the most presence. The rest can step back.', 'Keep the source accessible, but let me decide when to open it.'] },
  { id: 'week', title: 'Things to carry into next week', kind: 'Note', time: 'Yesterday · 6:18 PM', excerpt: 'A little space to think before doing.', summary: 'Leave room in the week for the work that does not come with a meeting invite. A few longer stretches of uninterrupted time can make the rest feel less fragmented.', highlight: 'Protect a little space to think.', tasks: ['Set aside one afternoon for focused work'], source: [] },
];

export default function App() {
  const [accent, setAccent] = useState<NeonAccent>(DEFAULT_NEON_ACCENT);
  const [mode, setMode] = useState<ThemeMode>('light');
  const [view, setView] = useState<'product' | 'system'>('product');
  const [notes, setNotes] = useState(INITIAL_NOTES);
  const [selected, setSelected] = useState('product');
  const [filter, setFilter] = useState<'all' | 'Meeting' | 'Note'>('all');
  const [query, setQuery] = useState('');
  const [source, setSource] = useState(false);
  const [sourceLine, setSourceLine] = useState(1);
  const [checked, setChecked] = useState<Record<string, boolean>>({});
  const [draft, setDraft] = useState(false);
  const [draftTitle, setDraftTitle] = useState('');
  const [draftBody, setDraftBody] = useState('');
  const [status, setStatus] = useState('');
  const [mobileReader, setMobileReader] = useState(false);
  const [componentValue, setComponentValue] = useState('Product conversation');
  const [componentChecked, setComponentChecked] = useState(false);
  const [componentState, setComponentState] = useState('');
  const titleRef = useRef<HTMLInputElement>(null);
  const sourceRef = useRef<HTMLElement>(null);
  const note = notes.find(item => item.id === selected) ?? notes[0];
  const tokens = getNeonTokens(accent, mode);
  const filtered = notes.filter(item => (filter === 'all' || item.kind === filter) && `${item.title} ${item.summary}`.toLowerCase().includes(query.toLowerCase()));

  useEffect(() => { if (draft) titleRef.current?.focus(); }, [draft]);
  useEffect(() => {
    if (source && window.matchMedia('(max-width: 1240px)').matches) {
      sourceRef.current?.scrollIntoView({ block: 'start' });
    }
  }, [source]);

  function openNote(id: string) { setSelected(id); setDraft(false); setSource(false); setStatus(''); setMobileReader(true); }
  function newNote() { setDraft(true); setDraftTitle(''); setDraftBody(''); setSource(false); setStatus(''); setMobileReader(true); }
  function saveNote() {
    if (!draftTitle.trim() || !draftBody.trim()) return;
    const id = `draft-${Date.now()}`;
    setNotes(items => [{ id, title: draftTitle.trim(), kind: 'Note', time: 'Just now', excerpt: draftBody.trim(), summary: draftBody.trim(), highlight: '', tasks: [], source: [] }, ...items]);
    setSelected(id); setDraft(false); setFilter('all'); setQuery(''); setStatus('Saved in this preview.');
  }
  function selectFilter(next: typeof filter) {
    setFilter(next); setMobileReader(false); setDraft(false); setSource(false); setStatus('');
    const first = notes.find(item => next === 'all' || item.kind === next);
    if (first) setSelected(first.id);
  }

  return <div className="nd-system nd-studio" data-mode={mode} style={neonCssVariables(accent, mode) as CSSProperties}>
    <header className="studio-toolbar">
      <div className="studio-title"><span>noted</span><span className="studio-divider">/</span><span>Product design</span></div>
      <div className="studio-views" role="group" aria-label="Preview view">
        <button aria-pressed={view === 'product'} onClick={() => setView('product')}>Workspace</button>
        <button aria-pressed={view === 'system'} onClick={() => setView('system')}>Design system</button>
      </div>
      <div className="studio-options">
        <div className="accent-picker" role="group" aria-label="Neon accent">
          {(Object.entries(NEON_ACCENTS) as [NeonAccent, typeof NEON_ACCENTS[NeonAccent]][]).map(([id, value]) => <button key={id} aria-label={value.name} title={value.name} aria-pressed={accent === id} onClick={() => setAccent(id)} style={{ '--swatch': value.vivid } as CSSProperties}><span>{accent === id && <Check size={13} strokeWidth={2.5} />}</span></button>)}
        </div>
        <select aria-label="Color mode" value={mode} onChange={e => setMode(e.target.value as ThemeMode)}><option value="light">Light</option><option value="dark">Dark</option></select>
      </div>
    </header>

    {view === 'product' ? <div className={`workspace ${mobileReader ? 'show-reader' : ''}`}>
      <aside className="product-sidebar">
        <img src={wordmark} alt="noted" className="product-wordmark" />
        <Button variant="accent" onClick={newNote}><Plus size={16} />New note</Button>
        <nav aria-label="Notes navigation">
          <button aria-current={filter === 'all' ? 'page' : undefined} onClick={() => selectFilter('all')}><Layers size={17} />All notes<span>{notes.length}</span></button>
          <button aria-current={filter === 'Meeting' ? 'page' : undefined} onClick={() => selectFilter('Meeting')}><Mic size={17} />Meetings</button>
          <button aria-current={filter === 'Note' ? 'page' : undefined} onClick={() => selectFilter('Note')}><FileText size={17} />Personal notes</button>
        </nav>
        <div className="sidebar-bottom"><LockKeyhole size={14} /><span>Your space.<br /><small>Sample notes for this preview</small></span></div>
      </aside>

      <section className="note-index" aria-label="Note library">
        <div className="index-heading"><h1>{filter === 'all' ? 'All notes' : filter === 'Meeting' ? 'Meetings' : 'Personal notes'}</h1><span>{filtered.length}</span></div>
        <label className="note-search"><Search size={15} /><input type="search" aria-label="Search notes" placeholder="Find a thought…" value={query} onChange={e => setQuery(e.target.value)} /></label>
        <div className="note-list">
          {filtered.length ? filtered.map(item => <button key={item.id} className={`note-row ${selected === item.id && !draft ? 'selected' : ''}`} aria-pressed={selected === item.id && !draft} onClick={() => openNote(item.id)}>
            <span className="note-row-meta">{item.kind === 'Meeting' ? <Mic size={12} /> : <FileText size={12} />}{item.time}</span>
            <strong>{item.title}</strong><span className="note-excerpt">{item.excerpt}</span>
          </button>) : <div className="empty-search"><Search size={22} /><h2>No notes found</h2><p>Try another word or clear your search.</p><Button variant="quiet" onClick={() => setQuery('')}>Clear search</Button></div>}
        </div>
        <div className="index-footer">A thought. Kept.</div>
      </section>

      <main className="note-main">
        <div className="document-toolbar"><button className="mobile-back" onClick={() => setMobileReader(false)}><ArrowLeft size={16} />Notes</button><div className="breadcrumbs"><span>My notes</span><ChevronRight size={13} /><span>{draft ? 'New note' : note.kind}</span></div><span className="toolbar-spacer" />{!draft && note.source.length > 0 && <Button variant="quiet" aria-expanded={source} onClick={() => setSource(!source)}><PanelRight size={15} />Source</Button>}</div>
        <div className={`reading-layout ${source ? 'with-source' : ''}`}>
          <article className="note-document">
            {draft ? <form onSubmit={e => { e.preventDefault(); saveNote(); }} className="draft-form">
              <p className="document-meta">Personal note</p><h1>Room for a thought.</h1>
              <label className="nd-field" htmlFor="draft-title"><span>Title</span><input ref={titleRef} id="draft-title" placeholder="Give it a name" value={draftTitle} onChange={e => setDraftTitle(e.target.value)} required /></label>
              <label className="nd-field" htmlFor="draft-body"><span>Note</span><textarea id="draft-body" rows={9} placeholder="What’s on your mind?" value={draftBody} onChange={e => setDraftBody(e.target.value)} required /></label>
              <div className="draft-actions"><Button type="submit" disabled={!draftTitle.trim() || !draftBody.trim()}>Save note</Button><Button variant="quiet" onClick={() => setDraft(false)}>Cancel</Button></div>
            </form> : <>
              <div className="document-meta">{note.kind === 'Meeting' ? <Mic size={14} /> : <FileText size={14} />}<span>{note.time}</span>{note.kind === 'Meeting' && <span>28 min</span>}</div>
              <h1>{note.title}</h1>
              {note.kind === 'Meeting' && <p className="document-deck">{note.excerpt}</p>}
              <div className="document-section"><h2>{note.kind === 'Meeting' ? 'The conversation' : 'The thought'}</h2><p>{note.summary}</p></div>
              {note.highlight && <div className="key-thought"><p><mark>{note.highlight}</mark></p>{note.source.length > 0 && <Citation onClick={() => { setSource(true); setSourceLine(1); }}>View source · 12:40</Citation>}</div>}
              {note.tasks.length > 0 && <div className="document-section next-steps"><h2>Carry it forward</h2>{note.tasks.map((task, index) => <CheckField key={`${note.id}-${index}`} checked={!!checked[`${note.id}-${index}`]} onChange={e => setChecked({ ...checked, [`${note.id}-${index}`]: e.target.checked })}>{task}</CheckField>)}</div>}
              <footer className="document-footer"><span>{note.kind === 'Meeting' ? 'Connected to the original conversation' : 'A little more context, kept.'}</span>{note.source.length > 0 && <Citation onClick={() => setSource(true)}>3 source passages</Citation>}</footer>
            </>}
            <div className="save-status" role="status">{status}</div>
          </article>
          {source && <aside ref={sourceRef} className="source-panel" aria-label="Original conversation"><div className="source-heading"><h2>Original conversation</h2><Button variant="quiet" aria-label="Close source" onClick={() => setSource(false)}><X size={16} /></Button></div><p className="source-caption">{note.title} · Transcript</p>{note.source.map((text, index) => <button key={text} className={`source-passage ${sourceLine === index ? 'active' : ''}`} aria-pressed={sourceLine === index} onClick={() => setSourceLine(index)}><span>{['12:18', '12:40', '13:06'][index]}</span><p>{text}</p></button>)}<div className="source-bottom"><LockKeyhole size={13} />Sample transcript</div></aside>}
        </div>
      </main>
    </div> : <main className="system-page">
      <section className="system-intro"><div><h1>One identity.<br />Any neon.</h1><p>A monochrome workspace. One vivid thread.<br />The same components, whatever your color.</p></div><div className="signature-swatch" style={{ background: tokens.accent, color: tokens.onAccent }}><span>{NEON_ACCENTS[accent].name}</span><strong>Aa</strong><code>{tokens.accent}</code></div></section>
      <section className="system-section"><h2>Color with a job</h2><div className="token-grid">{([
        ['Canvas', tokens.canvas, tokens.ink], ['Ink', tokens.ink, tokens.canvas], ['Accent', tokens.accent, tokens.onAccent], ['Accent text', tokens.accentInk, tokens.canvas], ['Selection', tokens.accentSoft, tokens.accentInk], ['Focus', tokens.focus, tokens.canvas],
      ] as const).map(([name, fill, ink]) => <div className="token-sample" key={name}><div style={{ background: fill, color: ink }}>Aa</div><strong>{name}</strong><code>{fill}</code></div>)}</div><p className="system-note">Bright fills pair with near-black text. Links and focus use the readable companion shade. Error and success keep their meaning across accents.</p></section>
      <section className="system-section"><h2>Built for the everyday</h2><div className="component-grid"><div className="component-example"><h3>Actions</h3><div className="component-actions"><Button onClick={() => setComponentState('Note saved.')}><Check size={14} />Save note</Button><Button variant="accent" onClick={() => setComponentState('Capture ready.')}><Plus size={15} />New note</Button><Button disabled>Saving…</Button></div><p className="component-status" role="status">{componentState || 'Primary, accent, disabled'}</p></div><div className="component-example"><h3>Capture</h3><TextField id="sample-title" label="Note title" value={componentValue} onChange={e => setComponentValue(e.target.value)} error={!componentValue.trim() ? 'Give your note a title.' : undefined} hint="A name you’ll recognize later." /></div><div className="component-example"><h3>Meaningful emphasis</h3><p className="sample-highlight"><mark>Start with one clear action.</mark></p><Citation onClick={() => { setView('product'); openNote('product'); setSource(true); }}>View source · 12:40</Citation></div><div className="component-example"><h3>Follow-through</h3><CheckField checked={componentChecked} onChange={e => setComponentChecked(e.target.checked)}>Review the first capture experience</CheckField><div className="semantic-messages"><span className="success-message"><Check size={13} />Saved</span><span className="error-message">Couldn’t save. Try again.</span></div></div></div></section>
      <section className="system-section type-section"><div><h2>Quiet type. Clear hierarchy.</h2><p className="system-note">Geist for the product. The Soft Cut wordmark for the brand. Reading text stays comfortable; labels stay useful.</p></div><div className="type-specimens"><span className="type-title">A thought. Kept.</span><span className="type-section-title">The conversation</span><span className="type-reading">Keep the thought and the thread.</span><span className="type-label">Product conversation · 28 min</span></div></section>
      <section className="system-section"><h2>Space to think</h2><div className="spacing-samples">{FOUNDATIONS.spacing.map(space => <div key={space}><i style={{ width: space, height: 16 }} /><code>{space}</code></div>)}</div><p className="system-note">6 px controls · 10 px panels · 16 px sheets. Motion settles in 120–180 ms and respects reduced motion.</p></section>
      <section className="system-section brand-section"><div><h2>The world around the product</h2><p className="system-note">The archive and its connecting thread belong in brand moments. The reading surface gives that space back to your notes.</p><p className="system-note">Approved visual direction · Citron reference</p></div><img src={reference} alt="Noted brand exploration showing the archive orbit, neon citron, notebook, stationery, and desktop concept" /></section>
      <footer className="system-footer">Noted design system · Foundation study <ArrowUpRight size={15} /></footer>
    </main>}
  </div>;
}

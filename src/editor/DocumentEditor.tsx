import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  EditorContent,
  NodeViewWrapper,
  ReactNodeViewRenderer,
  useEditor,
  type NodeViewProps,
} from "@tiptap/react";
import type { JSONContent } from "@tiptap/core";
import { StarterKit } from "@tiptap/starter-kit";
import { Image } from "@tiptap/extension-image";
import { TaskItem, TaskList } from "@tiptap/extension-list";
import { Placeholder } from "@tiptap/extensions";
import {
  Bold as BoldIcon,
  Code2,
  ImageOff,
  ImagePlus,
  Italic as ItalicIcon,
  List,
  ListTree,
  ListOrdered,
  ListTodo,
  Pilcrow,
  Quote,
  Redo2,
  Strikethrough,
  Underline as UnderlineIcon,
  Undo2,
  type LucideIcon,
} from "lucide-react";
import { api } from "../api";
import { fileToImg } from "../image";
import { documentFingerprint, type StructuredDocument } from "./document";

const MAX_EDITOR_IMAGE_BYTES = 25 * 1024 * 1024;
const EDITOR_IMAGE_EXTENSIONS = /\.(avif|gif|heic|heif|jpe?g|png|webp)$/i;

function StoredImageView({ node, selected }: NodeViewProps) {
  const localPath = typeof node.attrs.localPath === "string" ? node.attrs.localPath : "";
  const directSource = typeof node.attrs.src === "string" ? node.attrs.src : "";
  const alt = typeof node.attrs.alt === "string" && node.attrs.alt.trim() ? node.attrs.alt : "Image";
  const [source, setSource] = useState(localPath ? "" : directSource);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let active = true;
    setFailed(false);
    if (!localPath) {
      setSource(directSource);
      return () => { active = false; };
    }
    setSource("");
    api.loadImage(localPath)
      .then((image) => {
        if (active) setSource(`data:${image.mimeType};base64,${image.dataBase64}`);
      })
      .catch(() => {
        if (active) setFailed(true);
      });
    return () => { active = false; };
  }, [directSource, localPath]);

  return (
    <NodeViewWrapper
      as="figure"
      className={`noted-editor-image${selected ? " selected" : ""}`}
      data-drag-handle="true"
      contentEditable={false}
    >
      {source ? (
        <img src={source} alt={alt} draggable={false} />
      ) : (
        <div className={`noted-editor-image-placeholder${failed ? " failed" : ""}`}>
          <ImageOff size={18} aria-hidden="true" />
          <span>{failed ? "Image unavailable" : "Loading image…"}</span>
        </div>
      )}
    </NodeViewWrapper>
  );
}

const StoredImage = Image.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      localPath: {
        default: null,
        rendered: false,
      },
    };
  },
  addNodeView() {
    return ReactNodeViewRenderer(StoredImageView);
  },
}).configure({
  allowBase64: false,
});

type EditorButtonProps = {
  label: string;
  icon: LucideIcon;
  active?: boolean;
  disabled?: boolean;
  onClick: () => void;
  shortcut?: string;
};

function EditorButton({ label, icon: Icon, active, disabled = false, onClick, shortcut }: EditorButtonProps) {
  const title = shortcut ? `${label} (${shortcut})` : label;
  return (
    <button
      type="button"
      className={active === true ? "on" : ""}
      aria-label={label}
      aria-pressed={active === undefined ? undefined : active}
      disabled={disabled}
      title={title}
      onMouseDown={(event) => event.preventDefault()}
      onClick={onClick}
    >
      <Icon size={15} strokeWidth={1.9} />
    </button>
  );
}

type DocumentHeading = {
  level: number;
  position: number;
  text: string;
};

export function DocumentEditor({
  value,
  onChange,
  placeholder = "Start writing…",
  ariaLabel = "Document editor",
  autoFocus = false,
  disabled = false,
  variant = "compact",
  pageHeader,
}: {
  value: StructuredDocument;
  onChange: (document: StructuredDocument) => void;
  placeholder?: string;
  ariaLabel?: string;
  autoFocus?: boolean;
  disabled?: boolean;
  variant?: "compact" | "page";
  pageHeader?: ReactNode;
}) {
  const onChangeRef = useRef(onChange);
  const disabledRef = useRef(disabled);
  const imageInputRef = useRef<HTMLInputElement>(null);
  const insertImagesRef = useRef<(files: File[], position?: number) => void>(() => {});
  const [toolbarRevision, setToolbarRevision] = useState(0);
  const [imageBusy, setImageBusy] = useState(false);
  const [imageDragging, setImageDragging] = useState(false);
  const [imageError, setImageError] = useState<string | null>(null);
  onChangeRef.current = onChange;
  disabledRef.current = disabled;

  const editor = useEditor({
    extensions: [
      StarterKit.configure({
        heading: { levels: [1, 2, 3] },
        horizontalRule: false,
      }),
      TaskList.configure({
        HTMLAttributes: { class: "noted-task-list" },
      }),
      TaskItem.configure({
        nested: true,
        HTMLAttributes: { class: "noted-task-item" },
        a11y: {
          checkboxLabel: (node, checked) =>
            `${checked ? "Mark incomplete" : "Mark complete"}: ${node.firstChild?.textContent || node.textContent || "empty task"}`,
        },
      }),
      StoredImage,
      Placeholder.configure({
        placeholder,
        includeChildren: true,
        showOnlyCurrent: true,
      }),
    ],
    content: value as JSONContent,
    editable: !disabled,
    editorProps: {
      attributes: {
        class: "noted-document-content",
        role: "textbox",
        "aria-label": ariaLabel,
        "aria-multiline": "true",
        spellcheck: "true",
      },
      handlePaste: (_view, event) => {
        const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
          file.type.startsWith("image/") || EDITOR_IMAGE_EXTENSIONS.test(file.name)
        );
        if (!files.length) return false;
        event.preventDefault();
        insertImagesRef.current(files);
        return true;
      },
      handleDOMEvents: {
        dragover: (_view, event) => {
          const files = Array.from(event.dataTransfer?.items ?? []);
          const hasImage = files.some((item) => item.kind === "file" && item.type.startsWith("image/"));
          if (!hasImage) return false;
          event.preventDefault();
          setImageDragging(true);
          return true;
        },
        dragleave: (_view, event) => {
          const target = event.currentTarget as HTMLElement | null;
          if (!target?.contains(event.relatedTarget as Node | null)) setImageDragging(false);
          return false;
        },
        drop: (view, event) => {
          const files = Array.from(event.dataTransfer?.files ?? []).filter((file) =>
            file.type.startsWith("image/") || EDITOR_IMAGE_EXTENSIONS.test(file.name)
          );
          setImageDragging(false);
          if (!files.length) return false;
          event.preventDefault();
          const position = view.posAtCoords({ left: event.clientX, top: event.clientY })?.pos;
          insertImagesRef.current(files, position);
          return true;
        },
      },
    },
    onUpdate: ({ editor: currentEditor }) => {
      onChangeRef.current(currentEditor.getJSON() as StructuredDocument);
      setToolbarRevision((revision) => revision + 1);
    },
    onSelectionUpdate: () => setToolbarRevision((revision) => revision + 1),
  });

  insertImagesRef.current = (files, position) => {
    if (!editor || disabledRef.current || !files.length) return;
    void (async () => {
      setImageBusy(true);
      setImageError(null);
      try {
        let insertionPosition = position;
        for (const file of files) {
          if (file.size > MAX_EDITOR_IMAGE_BYTES) {
            throw new Error(`${file.name || "Image"} is larger than 25 MB.`);
          }
          if (!file.type.startsWith("image/") && !EDITOR_IMAGE_EXTENSIONS.test(file.name)) {
            throw new Error(`${file.name || "That file"} is not a supported image.`);
          }
          const image = await fileToImg(file);
          const localPath = await api.saveImage(image.base64, image.ext);
          let chain = editor.chain().focus();
          if (typeof insertionPosition === "number") chain = chain.setTextSelection(insertionPosition);
          chain.insertContent({
            type: "image",
            attrs: {
              src: localPath,
              localPath,
              alt: file.name || "Image",
            },
          }).run();
          insertionPosition = undefined;
        }
      } catch (reason) {
        setImageError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        setImageBusy(false);
      }
    })();
  };

  const incomingFingerprint = documentFingerprint(value);
  useEffect(() => {
    if (!editor) return;
    const current = editor.getJSON() as StructuredDocument;
    if (documentFingerprint(current) !== incomingFingerprint) {
      editor.commands.setContent(value as JSONContent, { emitUpdate: false });
      setToolbarRevision((revision) => revision + 1);
    }
  }, [editor, incomingFingerprint, value]);

  useEffect(() => {
    editor?.setEditable(!disabled);
  }, [disabled, editor]);

  useEffect(() => {
    if (!editor || !autoFocus) return;
    const frame = window.requestAnimationFrame(() => editor.commands.focus("end"));
    return () => window.cancelAnimationFrame(frame);
  }, [autoFocus, editor]);

  // Selection changes update this counter so active marks and undo availability
  // stay accurate without making the document value itself a toolbar concern.
  void toolbarRevision;
  const canUndo = editor?.can().undo() ?? false;
  const canRedo = editor?.can().redo() ?? false;

  let textStyle = "paragraph";
  if (editor?.isActive("heading", { level: 1 })) textStyle = "heading-1";
  else if (editor?.isActive("heading", { level: 2 })) textStyle = "heading-2";
  else if (editor?.isActive("heading", { level: 3 })) textStyle = "heading-3";

  const headings: DocumentHeading[] = [];
  editor?.state.doc.descendants((node, position) => {
    if (node.type.name !== "heading") return;
    const text = node.textContent.trim();
    if (!text) return;
    headings.push({
      level: Number(node.attrs.level) || 1,
      position,
      text,
    });
  });

  function setTextStyle(style: string) {
    if (!editor) return;
    if (style === "heading-1") editor.chain().focus().toggleHeading({ level: 1 }).run();
    else if (style === "heading-2") editor.chain().focus().toggleHeading({ level: 2 }).run();
    else if (style === "heading-3") editor.chain().focus().toggleHeading({ level: 3 }).run();
    else editor.chain().focus().setParagraph().run();
  }

  const toolbar = (
    <div className="document-editor-toolbar" role="toolbar" aria-label="Text formatting">
      {variant === "page" ? (
        <label className="document-editor-style">
          <span className="sr-only">Text style</span>
          <select
            aria-label="Text style"
            value={textStyle}
            disabled={!editor || disabled}
            onChange={(event) => setTextStyle(event.target.value)}
          >
            <option value="paragraph">Text</option>
            <option value="heading-1">Heading 1</option>
            <option value="heading-2">Heading 2</option>
            <option value="heading-3">Heading 3</option>
          </select>
        </label>
      ) : (
        <>
          <EditorButton
            label="Paragraph"
            icon={Pilcrow}
            active={editor?.isActive("paragraph") && !editor?.isActive("taskList")}
            disabled={!editor || disabled}
            onClick={() => editor?.chain().focus().setParagraph().run()}
          />
          <span className="document-editor-divider" aria-hidden="true" />
        </>
      )}
      <EditorButton
        label="Bold"
        icon={BoldIcon}
        active={editor?.isActive("bold")}
        disabled={!editor || disabled}
        shortcut="⌘B"
        onClick={() => editor?.chain().focus().toggleBold().run()}
      />
      <EditorButton
        label="Italic"
        icon={ItalicIcon}
        active={editor?.isActive("italic")}
        disabled={!editor || disabled}
        shortcut="⌘I"
        onClick={() => editor?.chain().focus().toggleItalic().run()}
      />
      <EditorButton
        label="Underline"
        icon={UnderlineIcon}
        active={editor?.isActive("underline")}
        disabled={!editor || disabled}
        shortcut="⌘U"
        onClick={() => editor?.chain().focus().toggleUnderline().run()}
      />
      <EditorButton
        label="Strikethrough"
        icon={Strikethrough}
        active={editor?.isActive("strike")}
        disabled={!editor || disabled}
        shortcut="⌘⇧S"
        onClick={() => editor?.chain().focus().toggleStrike().run()}
      />
      <span className="document-editor-divider" aria-hidden="true" />
      <EditorButton
        label="Checklist"
        icon={ListTodo}
        active={editor?.isActive("taskList")}
        disabled={!editor || disabled}
        onClick={() => editor?.chain().focus().toggleTaskList().run()}
      />
      <EditorButton
        label="Bulleted list"
        icon={List}
        active={editor?.isActive("bulletList")}
        disabled={!editor || disabled}
        shortcut="⌘⇧8"
        onClick={() => editor?.chain().focus().toggleBulletList().run()}
      />
      <EditorButton
        label="Numbered list"
        icon={ListOrdered}
        active={editor?.isActive("orderedList")}
        disabled={!editor || disabled}
        shortcut="⌘⇧7"
        onClick={() => editor?.chain().focus().toggleOrderedList().run()}
      />
      {variant === "page" && (
        <>
          <EditorButton
            label="Quote"
            icon={Quote}
            active={editor?.isActive("blockquote")}
            disabled={!editor || disabled}
            onClick={() => editor?.chain().focus().toggleBlockquote().run()}
          />
          <EditorButton
            label="Code block"
            icon={Code2}
            active={editor?.isActive("codeBlock")}
            disabled={!editor || disabled}
            onClick={() => editor?.chain().focus().toggleCodeBlock().run()}
          />
        </>
      )}
      <EditorButton
        label="Add image"
        icon={ImagePlus}
        disabled={!editor || disabled || imageBusy}
        onClick={() => imageInputRef.current?.click()}
      />
      <span className="document-editor-spacer" />
      <EditorButton
        label="Undo"
        icon={Undo2}
        disabled={!editor || disabled || !canUndo}
        shortcut="⌘Z"
        onClick={() => editor?.chain().focus().undo().run()}
      />
      <EditorButton
        label="Redo"
        icon={Redo2}
        disabled={!editor || disabled || !canRedo}
        shortcut="⌘⇧Z"
        onClick={() => editor?.chain().focus().redo().run()}
      />
    </div>
  );

  const canvas = (
    <div className={`document-editor-canvas${imageDragging ? " image-dragging" : ""}`}>
      <EditorContent editor={editor} />
      {imageDragging && <div className="document-editor-drop-hint">Drop image here</div>}
    </div>
  );

  const imageStatus = (imageBusy || imageError) && (
    <div className={`document-editor-image-status${imageError ? " error" : ""}`} role="status">
      {imageError ?? "Adding image…"}
    </div>
  );

  return (
    <div className={`document-editor ${variant}${disabled ? " disabled" : ""}`}>
      {toolbar}
      {variant === "page" ? (
        <div className="document-editor-stage">
          <div className="document-editor-stage-inner">
            <aside className="document-editor-outline" aria-label="Document outline">
              <div className="document-editor-outline-label">
                <ListTree size={14} aria-hidden="true" />
                <span>Outline</span>
              </div>
              {headings.length ? (
                <nav>
                  {headings.map((heading, index) => (
                    <button
                      key={`${heading.position}-${index}`}
                      type="button"
                      className={`level-${heading.level}`}
                      title={heading.text}
                      onClick={() => {
                        editor?.chain().focus().setTextSelection(heading.position + 1).scrollIntoView().run();
                      }}
                    >
                      {heading.text}
                    </button>
                  ))}
                </nav>
              ) : (
                <p>Headings will appear here.</p>
              )}
            </aside>
            <section className="document-editor-page" aria-label="Document page">
              {pageHeader}
              {canvas}
              {imageStatus}
            </section>
            <div className="document-editor-balance" aria-hidden="true" />
          </div>
        </div>
      ) : (
        <>
          {canvas}
          {imageStatus}
        </>
      )}
      <input
        ref={imageInputRef}
        className="document-editor-file-input"
        type="file"
        accept="image/png,image/jpeg,image/gif,image/webp,image/heic,image/heif,image/avif"
        multiple
        tabIndex={-1}
        onChange={(event) => {
          insertImagesRef.current(Array.from(event.target.files ?? []));
          event.target.value = "";
        }}
      />
    </div>
  );
}

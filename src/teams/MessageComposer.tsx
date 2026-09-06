import {
  forwardRef,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
} from "react";
import { Schema } from "@tiptap/pm/model";
import { EditorState, Plugin, TextSelection } from "@tiptap/pm/state";
import { Decoration, DecorationSet, EditorView } from "@tiptap/pm/view";
import { history, undo, redo } from "@tiptap/pm/history";
import { keymap } from "@tiptap/pm/keymap";
import { messageFormatting } from "./messageFormatting";
import "./message-composer.css";

const schema = new Schema({
  nodes: {
    doc: { content: "text*", whitespace: "pre" },
    text: { group: "inline" },
  },
});
export type MessageComposerHandle = {
  focus: (options?: FocusOptions) => void;
  setSelectionRange: (start: number, end: number) => void;
};
type Props = {
  value: string;
  disabled: boolean;
  placeholder: string;
  id: string;
  onChange: (value: string, caret: number) => void;
  onSelect: (caret: number) => void;
  onKeyDown: (event: KeyboardEvent) => void;
  "aria-controls"?: string;
  "aria-activedescendant"?: string;
  "aria-describedby"?: string;
};
/** Plain-text document + visual decorations: Markdown stays lossless in drafts and on the wire. */
export const MessageComposer = forwardRef<MessageComposerHandle, Props>(
  (props, ref) => {
    const host = useRef<HTMLDivElement>(null);
    const view = useRef<EditorView | null>(null);
    const latest = useRef(props);
    latest.current = props;
    useImperativeHandle(
      ref,
      () => ({
        focus: (options) => view.current?.dom.focus(options),
        setSelectionRange: (start, end) => {
          const editor = view.current;
          if (editor)
            editor.dispatch(
              editor.state.tr.setSelection(
                TextSelection.create(
                  editor.state.doc,
                  Math.min(start, editor.state.doc.content.size),
                  Math.min(end, editor.state.doc.content.size),
                ),
              ),
            );
        },
      }),
      [],
    );
    useLayoutEffect(() => {
      const editor = new EditorView(host.current!, {
        state: EditorState.create({
          schema,
          doc: schema.node(
            "doc",
            null,
            props.value ? schema.text(props.value) : undefined,
          ),
          plugins: [
            new Plugin({
              filterTransaction: (transaction) =>
                transaction.doc.content.size <= 10_000,
            }),
            history(),
            keymap({ "Mod-z": undo, "Mod-Shift-z": redo, "Mod-y": redo }),
          ],
        }),
        editable: () => !latest.current.disabled,
        decorations: (state) =>
          DecorationSet.create(
            state.doc,
            messageFormatting(state.doc.textContent).flatMap((span) => [
              Decoration.inline(span.start, span.contentStart, {
                class: "message-format-marker",
              }),
              Decoration.inline(span.contentStart, span.contentEnd, {
                class: `message-format-${span.kind}`,
              }),
              Decoration.inline(span.contentEnd, span.end, {
                class: "message-format-marker",
              }),
            ]),
          ),
        dispatchTransaction: (transaction) => {
          editor.updateState(editor.state.apply(transaction));
          if (transaction.docChanged)
            latest.current.onChange(
              editor.state.doc.textContent,
              editor.state.selection.head,
            );
          else latest.current.onSelect(editor.state.selection.head);
        },
        handleKeyDown: (_view, event) => {
          if (editor.composing || event.isComposing || event.keyCode === 229)
            return false;
          latest.current.onKeyDown(event);
          if (event.defaultPrevented) return true;
          if (!event.isComposing && event.key === "Enter") {
            editor.dispatch(editor.state.tr.insertText("\n"));
            return true;
          }
          return false;
        },
        handlePaste: (_view, event) => {
          const data = event.clipboardData;
          if (!data || data.files.length) return false;
          const available =
            10_000 -
            editor.state.doc.content.size +
            editor.state.selection.to -
            editor.state.selection.from;
          editor.dispatch(
            editor.state.tr.insertText(
              data
                .getData("text/plain")
                .replace(/\r\n?/g, "\n")
                .slice(0, available),
            ),
          );
          return true;
        },
        // Dropped files are staged by the conversation, never inserted into the document.
        handleDrop: () => true,
      });
      view.current = editor;
      return () => {
        view.current = null;
        editor.destroy();
      };
    }, []);
    useLayoutEffect(() => {
      const editor = view.current!;
      if (!props.value && editor.state.doc.textContent) {
        editor.updateState(
          EditorState.create({ schema, plugins: editor.state.plugins }),
        );
      } else if (editor.state.doc.textContent !== props.value) {
        editor.dispatch(
          editor.state.tr
            .replaceWith(
              0,
              editor.state.doc.content.size,
              props.value ? schema.text(props.value) : [],
            )
            .setMeta("addToHistory", false),
        );
      }
      editor.setProps({
        attributes: {
          id: props.id,
          role: "textbox",
          "aria-label": props.placeholder,
          "aria-multiline": "true",
          "aria-disabled": String(props.disabled),
          "aria-autocomplete": "list",
          "aria-controls": props["aria-controls"] ?? "",
          "aria-activedescendant": props["aria-activedescendant"] ?? "",
          "aria-describedby": props["aria-describedby"] ?? "",
          "data-placeholder": props.placeholder,
          "data-empty": String(!props.value),
        },
      });
    }, [
      props.value,
      props.disabled,
      props.placeholder,
      props.id,
      props["aria-controls"],
      props["aria-activedescendant"],
      props["aria-describedby"],
    ]);
    return <div className="message-composer-editor" ref={host} />;
  },
);

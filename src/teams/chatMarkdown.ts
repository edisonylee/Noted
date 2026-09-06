import type { Root, Element, RootContent } from "hast";

/** Links may leave the app only through ordinary web/mail protocols. */
export function chatMarkdownUrl(value: string, image = false) {
  if (!image && value.startsWith("#")) return value;
  try {
    const url = new URL(value);
    if (url.username || url.password) return "";
    return (
      image
        ? url.protocol === "https:"
        : ["https:", "http:", "mailto:"].includes(url.protocol)
    )
      ? url.href
      : "";
  } catch {
    return "";
  }
}

/** Decorate parsed text, never code or links; IDs stay scoped to one message. */
export function chatMarkdownText({ prefix }: { prefix: string }) {
  return (tree: Root) => {
    const walk = (parent: Root | Element) => {
      if (parent.type === "element") {
        if (parent.properties.id === "footnote-label")
          parent.properties.id = `${prefix}footnote-label`;
        if (Array.isArray(parent.properties.ariaDescribedBy))
          parent.properties.ariaDescribedBy =
            parent.properties.ariaDescribedBy.map((id) =>
              id === "footnote-label" ? `${prefix}footnote-label` : id,
            );
        if (["code", "pre", "a"].includes(parent.tagName)) return;
      }
      parent.children = parent.children.map((child): RootContent => {
        if (child.type === "text" && /[@#]/.test(child.value))
          return {
            type: "element",
            tagName: "span",
            properties: { dataChatText: true },
            children: [child],
          };
        if (child.type === "element") walk(child);
        return child;
      });
    };
    walk(tree);
  };
}

import { Switch, Match, type JSX } from "solid-js";
import type { ContentKind } from "./lib/ipc";

/** Shared monoline SVG frame — crisp at any size, inherits `currentColor`. */
function Svg(props: { children: JSX.Element; class?: string }) {
  return (
    <svg
      class={props.class}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      {props.children}
    </svg>
  );
}

/** Per-content-kind icon (replaces the old unicode glyphs). */
export function KindIcon(props: { kind: ContentKind }) {
  return (
    <Svg class="kind-icon">
      <Switch fallback={<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z" />}>
        <Match when={props.kind === "text"}>
          {/* Quotation mark — unmistakably "text", never a menu glyph. */}
          <path d="M7 8.5C5.3 9.3 4.5 10.7 4.5 12.7H7.8V16H4.5V12.7" />
          <path d="M15 8.5c-1.7.8-2.5 2.2-2.5 4.2H15.8V16H12.5V12.7" />
        </Match>
        <Match when={props.kind === "link"}>
          <path d="M9.5 14.5l5-5" />
          <path d="M11 6.8l1.2-1.2a3.8 3.8 0 0 1 5.4 5.4l-1.2 1.2" />
          <path d="M13 17.2l-1.2 1.2a3.8 3.8 0 0 1-5.4-5.4l1.2-1.2" />
        </Match>
        <Match when={props.kind === "code"}>
          <path d="M9 8l-4 4 4 4" />
          <path d="M15 8l4 4-4 4" />
        </Match>
        <Match when={props.kind === "rich_text"}>
          <rect x="5" y="4" width="14" height="16" rx="2" />
          <line x1="8.5" y1="9" x2="15.5" y2="9" />
          <line x1="8.5" y1="13" x2="13" y2="13" />
        </Match>
        <Match when={props.kind === "image"}>
          <rect x="3.5" y="5" width="17" height="14" rx="2.5" />
          <circle cx="8.5" cy="10" r="1.4" />
          <path d="M20.5 15.5l-4.5-4.5L6 19" />
        </Match>
        <Match when={props.kind === "files"}>
          <path d="M4 7.5a2 2 0 0 1 2-2h3.2l2 2H18a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2z" />
        </Match>
        <Match when={props.kind === "video"}>
          <rect x="3.5" y="6" width="17" height="12" rx="2.5" />
          <path d="M10.5 9.5l4.5 2.5-4.5 2.5z" fill="currentColor" stroke="none" />
        </Match>
        <Match when={props.kind === "audio"}>
          <path d="M9 17V6l10-2v11" />
          <circle cx="6.5" cy="17" r="2.4" />
          <circle cx="16.5" cy="15" r="2.4" />
        </Match>
      </Switch>
    </Svg>
  );
}

export function SearchIcon() {
  return (
    <Svg class="ui-icon">
      <circle cx="11" cy="11" r="7" />
      <line x1="16.5" y1="16.5" x2="21" y2="21" />
    </Svg>
  );
}

export function PinIcon() {
  return (
    <Svg class="ui-icon">
      <path d="M9.5 4h5l-1 6 3 2.5H7.5L10.5 10z" />
      <line x1="12" y1="15" x2="12" y2="20.5" />
    </Svg>
  );
}

export function TrashIcon() {
  return (
    <Svg class="ui-icon">
      <path d="M4.5 7h15" />
      <path d="M9 7V5h6v2" />
      <path d="M6.5 7l1 12.5h9L17.5 7" />
    </Svg>
  );
}

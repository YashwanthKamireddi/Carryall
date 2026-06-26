import { createSignal, createEffect, onMount, onCleanup, For, Show, Switch, Match } from "solid-js";
import {
  type Clip,
  type ContentKind,
  listClips,
  searchClips,
  copyClip,
  setPin,
  deleteClip,
  hidePalette,
  onNewClip,
} from "./lib/ipc";
import { KindIcon, SearchIcon, PinIcon, TrashIcon } from "./icons";
import "./theme.css";
import "./App.css";

const KIND_LABEL: Record<ContentKind, string> = {
  text: "Text", link: "Link", color: "Color", code: "Code", rich_text: "Rich text",
  image: "Image", files: "Files", video: "Video", audio: "Audio", binary: "Binary",
};

/** Real favicon for a link's domain (falls back to the link glyph on error). */
function faviconUrl(url: string): string | null {
  try {
    return `https://www.google.com/s2/favicons?domain=${new URL(url).hostname}&sz=64`;
  } catch {
    return null;
  }
}

function relativeTime(ms: number): string {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

export default function App() {
  const [query, setQuery] = createSignal("");
  const [clips, setClips] = createSignal<Clip[]>([]);
  const [selected, setSelected] = createSignal(0);
  let inputRef: HTMLInputElement | undefined;
  let scrollRef: HTMLDivElement | undefined;

  const refresh = async (q: string) => {
    const result = q.trim() ? await searchClips(q) : await listClips();
    setClips(result);
    setSelected(0);
  };

  let debounce: number | undefined;
  createEffect(() => {
    const q = query();
    clearTimeout(debounce);
    debounce = window.setTimeout(() => void refresh(q), 80);
  });

  onMount(async () => {
    await refresh("");
    inputRef?.focus();
    const unlisten = await onNewClip((clip) => {
      if (query().trim()) return;
      setClips((prev) => [clip, ...prev.filter((c) => c.id !== clip.id)]);
    });
    const onFocus = () => {
      setQuery("");
      void refresh("");
      inputRef?.focus();
      inputRef?.select();
    };
    window.addEventListener("focus", onFocus);
    onCleanup(() => {
      unlisten();
      window.removeEventListener("focus", onFocus);
    });
  });

  const commit = async (clip: Clip | undefined) => {
    if (clip) await copyClip(clip.id);
  };
  const move = (delta: number) => {
    const n = clips().length;
    if (n === 0) return;
    const next = Math.min(Math.max(selected() + delta, 0), n - 1);
    setSelected(next);
    scrollRef?.querySelector(`[data-idx="${next}"]`)?.scrollIntoView({ block: "nearest" });
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if ((e.ctrlKey || e.metaKey) && e.key >= "1" && e.key <= "9") {
      e.preventDefault();
      void commit(clips()[Number(e.key) - 1]);
      return;
    }
    switch (e.key) {
      case "ArrowDown": e.preventDefault(); move(1); break;
      case "ArrowUp": e.preventDefault(); move(-1); break;
      case "Enter": e.preventDefault(); void commit(clips()[selected()]); break;
      case "Escape": e.preventDefault(); void hidePalette(); break;
    }
  };
  const togglePin = async (clip: Clip, e: MouseEvent) => {
    e.stopPropagation();
    await setPin(clip.id, !clip.pinned);
    void refresh(query());
  };
  const remove = async (clip: Clip, e: MouseEvent) => {
    e.stopPropagation();
    await deleteClip(clip.id);
    setClips((prev) => prev.filter((c) => c.id !== clip.id));
  };

  const searching = () => query().trim().length > 0;
  const pinnedCount = () => clips().filter((c) => c.pinned).length;
  const pinned = () => clips().slice(0, pinnedCount());
  const recent = () => clips().slice(pinnedCount());

  const Cell = (clip: Clip, gi: number) => (
    <div
      class="cell"
      classList={{ selected: gi === selected() }}
      data-idx={gi}
      onMouseEnter={() => setSelected(gi)}
      onClick={() => void commit(clip)}
    >
      <div class="leadicon" style={{ background: `var(--c-${clip.kind})` }}>
        <Switch fallback={<span class="glyph"><KindIcon kind={clip.kind} /></span>}>
          <Match when={clip.kind === "image" && clip.thumb}>
            <img src={clip.thumb} alt="" />
          </Match>
          <Match when={clip.kind === "color"}>
            <span class="swatch" style={{ background: clip.preview }} />
          </Match>
          <Match when={clip.kind === "link" && faviconUrl(clip.preview)}>
            <span class="glyph"><KindIcon kind="link" /></span>
            <img
              class="favicon"
              src={faviconUrl(clip.preview)!}
              alt=""
              onError={(e) => (e.currentTarget.style.display = "none")}
            />
          </Match>
        </Switch>
      </div>
      <div class="celltext">
        <div class="ctitle" classList={{ mono: clip.kind === "code" }}>{clip.preview}</div>
        <div class="csub">{KIND_LABEL[clip.kind]} · {relativeTime(clip.created_at)}</div>
      </div>
      <div class="cellacc">
        <button class="acc" classList={{ on: clip.pinned }} title={clip.pinned ? "Unpin" : "Pin"}
          onClick={(e) => void togglePin(clip, e)}><PinIcon /></button>
        <button class="acc" title="Delete" onClick={(e) => void remove(clip, e)}><TrashIcon /></button>
      </div>
    </div>
  );

  return (
    <div class="ios-sheet" onKeyDown={onKeyDown}>
      <div class="nav">
        <span class="title">Clipboard</span>
        <span class="esc">esc</span>
      </div>
      <div class="search-wrap">
        <div class="search-field">
          <span class="icon"><SearchIcon /></span>
          <input
            ref={inputRef}
            placeholder="Search"
            spellcheck={false}
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
          />
        </div>
      </div>

      <div class="scroll" ref={scrollRef}>
        <Show
          when={clips().length > 0}
          fallback={<div class="empty">{searching() ? "No Results" : "No clips yet — copy something."}</div>}
        >
          <Show when={!searching()} fallback={
            <div class="group"><For each={clips()}>{(c, i) => Cell(c, i())}</For></div>
          }>
            <Show when={pinned().length > 0}>
              <div class="sec-header">Pinned</div>
              <div class="group"><For each={pinned()}>{(c, i) => Cell(c, i())}</For></div>
            </Show>
            <Show when={recent().length > 0}>
              <div class="sec-header">Recent</div>
              <div class="group"><For each={recent()}>{(c, i) => Cell(c, pinnedCount() + i())}</For></div>
            </Show>
          </Show>
        </Show>
      </div>
    </div>
  );
}

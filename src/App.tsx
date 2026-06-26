import { createSignal, createEffect, onMount, onCleanup, For, Show } from "solid-js";
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
  text: "Text",
  link: "Link",
  color: "Color",
  code: "Code",
  rich_text: "Rich text",
  image: "Image",
  files: "Files",
  video: "Video",
  audio: "Audio",
  binary: "Binary",
};

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
  let listRef: HTMLDivElement | undefined;

  const refresh = async (q: string) => {
    const result = q.trim() ? await searchClips(q) : await listClips();
    setClips(result);
    setSelected(0);
  };

  // Debounced search as the user types.
  let debounce: number | undefined;
  createEffect(() => {
    const q = query();
    clearTimeout(debounce);
    debounce = window.setTimeout(() => void refresh(q), 80);
  });

  onMount(async () => {
    await refresh("");
    inputRef?.focus();

    // Live capture pushes: if not searching, prepend the new clip.
    const unlisten = await onNewClip((clip) => {
      if (query().trim()) return;
      setClips((prev) => [clip, ...prev.filter((c) => c.id !== clip.id)]);
    });

    // When the palette is summoned (window regains focus), reset + focus.
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
    if (!clip) return;
    await copyClip(clip.id);
  };

  const move = (delta: number) => {
    const n = clips().length;
    if (n === 0) return;
    const next = Math.min(Math.max(selected() + delta, 0), n - 1);
    setSelected(next);
    listRef
      ?.querySelector(`[data-idx="${next}"]`)
      ?.scrollIntoView({ block: "nearest" });
  };

  const onKeyDown = (e: KeyboardEvent) => {
    // Ctrl/Cmd + 1–9 → instantly paste the Nth visible clip.
    if ((e.ctrlKey || e.metaKey) && e.key >= "1" && e.key <= "9") {
      e.preventDefault();
      const idx = Number(e.key) - 1;
      const target = clips()[idx];
      if (target) void commit(target);
      return;
    }
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Enter":
        e.preventDefault();
        void commit(clips()[selected()]);
        break;
      case "Escape":
        e.preventDefault();
        void hidePalette();
        break;
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

  const pinnedCount = () => clips().filter((c) => c.pinned).length;
  const searching = () => query().trim().length > 0;

  return (
    <div class="palette" onKeyDown={onKeyDown}>
      <div class="search">
        <span class="icon">
          <SearchIcon />
        </span>
        <input
          ref={inputRef}
          class="search-input"
          placeholder="Search your clipboard…"
          spellcheck={false}
          value={query()}
          onInput={(e) => setQuery(e.currentTarget.value)}
        />
        <kbd class="hint">esc</kbd>
      </div>

      <div class="list" ref={listRef}>
        <Show
          when={clips().length > 0}
          fallback={
            <div class="empty">
              <div class="blob" />
              {searching() ? "No matches." : "Nothing here yet — copy something."}
            </div>
          }
        >
          <For each={clips()}>
            {(clip, i) => (
              <>
                <Show when={!searching() && i() === 0 && clip.pinned}>
                  <div class="section-label">Pinned</div>
                </Show>
                <Show when={!searching() && i() === pinnedCount()}>
                  <div class="section-label">Recent</div>
                </Show>
                <div
                  class="row"
                  classList={{ selected: i() === selected() }}
                  data-idx={i()}
                  onMouseEnter={() => setSelected(i())}
                  onClick={() => void commit(clip)}
                >
                  <div class="thumb">
                  <Show
                    when={clip.kind === "image" && clip.thumb}
                    fallback={
                      <Show
                        when={clip.kind === "color"}
                        fallback={
                          <span class="glyph">
                            <KindIcon kind={clip.kind} />
                          </span>
                        }
                      >
                        <span class="swatch" style={{ background: clip.preview }} />
                      </Show>
                    }
                  >
                    <img src={clip.thumb} alt="" />
                  </Show>
                </div>
                <div class="body">
                  <div class="preview" classList={{ mono: clip.kind === "code" }}>
                    {clip.preview}
                  </div>
                  <div class="meta">
                    {KIND_LABEL[clip.kind]} · {relativeTime(clip.created_at)}
                  </div>
                </div>
                <div class="actions">
                  <button
                    class="act"
                    classList={{ on: clip.pinned }}
                    title={clip.pinned ? "Unpin" : "Pin"}
                    onClick={(e) => void togglePin(clip, e)}
                  >
                    <PinIcon />
                  </button>
                  <button
                    class="act danger"
                    title="Delete"
                    onClick={(e) => void remove(clip, e)}
                  >
                    <TrashIcon />
                  </button>
                  </div>
                </div>
              </>
            )}
          </For>
        </Show>
      </div>

      <div class="footer">
        <span class="keys">
          <kbd>↑</kbd>
          <kbd>↓</kbd> Navigate
        </span>
        <span class="keys">
          <kbd>↵</kbd> Paste
        </span>
        <span class="keys">
          <kbd>⌫</kbd> Delete
        </span>
      </div>
    </div>
  );
}

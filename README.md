# ce
brief editor with codeium ai support, vim modal core
Mini vim-buffer TUI editor with AI completions, split layouts, Brief mode, and dynamic configuration.

**Tech:** Rust 2021 · Ratatui + Crossterm · Codeium + Llama.cpp LLM · Vim-like Modal Sequences · Brief Mode (Always-Insert) · Tokio Async

---

## 📦 Module Architecture

### `main.rs` – CLI & Event Loop
- `clap` CLI: path args, `auth` subcommand, `status` subcommand.
- Tokio `mpsc` event bus: `AppMessage::Input` / `CompletionResponse` / `Tick`.
- Codeium server spawn + API key gated by `codeium_enabled` config flag – skipped when false.
- Request Version IDs protect against async completions overwriting newer state.
- Crossterm KeyPress‑only filtering (Release/Repeat events dropped at source).
- Local completion async task reads pre‑cached `buffer_words` + `vocab_words` + active line delta.

### `ed/` – Editor Core
- **buffer.rs** – Rope-backed Buffer, UndoSnapshot, file I/O, `rustfmt` on `.rs` save, bookmarks, Git change states.
- **window.rs** – Independent viewports, `desired_col` (curswant), LayoutNode binary tree, selection calculations.
- **editor.rs** – Request versioning, history I/O, buffer word cache refresh, QuitPrompt, Brief/Vim mode state.
- **movement.rs** – `hjkl`, word, line, page (viewport‑aware) movement.
- **editing.rs** – Auto‑indent Enter, soft‑tab, indent/outdent, paste helpers, AST updates.
- **repeat.rs** – Vim dot repeat (`.`) with `RepeatableAction` payloads and multipliers.
- **gutter.rs** – Absolute/relative line numbers, Git diff gutter signs, bookmarks.
- **syntax.rs** – Tree‑sitter integration, highlighting, text objects.
- **ext.rs** – Plugin hook stubs.

### `comp/` – Completion
- **state.rs** – CompletionMachine: ghost‑text, cycling, 400 ms throttle, request IDs.
- **provider.rs** – `CompletionProvider` async trait (backend‑agnostic).
- Contextual guards: blocks mid‑word, next‑to‑`)`, etc.
- `find_prefix_overlap()` prevents character duplication on accept.

### `ai/` – AI & LLM Backends
- **codeium/** – Cloud gRPC‑JSON completions, LSP helper thread, browser login auth.
- **llama/llm.rs** – Local `llama.cpp` integration over raw TCP stream (OpenAI‑compatible `/v1/chat/completions`).
- Powers both interactive split‑screen LLM chat (`:llm`) and conventional commit generator (`:gc` / status screen `c`).

### `render/` – UI
- **buffer_view.rs** – Renders windows with gutter, visual selection, query highlights.
- **tabs.rs** – Multi‑buffer tab bar (hidden when single buffer).
- **statusbar.rs** – Mode pill, filename (Fish‑style contraction), `[+]`, row:col, lang, scope, Codeium spinner.
- **command_line.rs** – `:` command + top‑5 candidate preview or status message (very bottom line).
- **helpers.rs** – CJK‑aware width, digit count, Fish‑style path contraction.

### `keybind/` – Keybindings
- **actions.rs** – `EditorAction` enum (pure data).
- **bindings.rs** – Action enum, `format_key`, `resolve_sequence`, `resolve_single_key`, Brief Alt‑key layer.
- Leader key dynamic multi‑key sequences with `mapleader` override.
- Namespaced config: `keybindings.normal`, `.insert`, `.visual`, `.global`.
- Text objects: `diw`, `ci"`, `di(`, `ci{`, etc.

### `config/` – Configuration
- **app_config.rs** – Load/save `~/.config/ce/config.json`.
- Fields: `api_key`, `api_url`, `portal_url`, `max_tokens`, `codeium_enabled`, `popup_enabled`, `init_mode`, `keybindings`, `leader`, `search_wrap_enabled`.
- Auto‑discovers nvim/Codeium key on load if `api_key` absent.

### `repl/` – Command REPL
- **command.rs** – `:q :q! :w :wq :x :e :new :ls :bn :bp :bd :b N :config :vocab :vim :brief :sp :vs :on :gs :stash`.
- Tab path completion with `./` and `../` prefix preservation.
- Command name prefix completion, persistent history search.

### `popup/` – Popup System
- **mod.rs** – `PopupState`, `PopupItem`, `PopupKind` (Completion/CommandPalette/Hover/Custom).
- Centered floating config dashboard – runtime Serde JSON bool reflection + Space toggle.
- Bottom‑right Which‑Key hint with compact modifier notation, synced with theme.

### `git/` – Git State Machinery
- **status.rs** – Git status screen parser (retains raw 2‑character columns).
- **PendingGitAction** – Safety intercept dialog state‑machine (checkout, pop warnings).
- Automatic file reloading, syntax parsing, gutter diff updates, cursor clamping after checkouts.

## ⚡ Event Flow

1. **Crossterm Key Event (Press Only)**  
   → `mpsc::Sender<AppMessage::Input>`  
   → Tokio Event Loop  
   → `Editor::handle_key()`

2. **50 ms Tick Timer**  
   → `AppMessage::Tick` → `poll_completion()`  
   → Codeium LSP or Local Scanner  
   → `tokio::spawn(fetch_completion_items())`

3. **`CompletionResponse` (ID verified)**  
   → `mpsc::Sender<AppMessage::CompletionResponse>`  
   → `Editor::ingest_completion_response()`  
   → `render::draw()`

> KeyPress‑only filter at input thread. Stale completions discarded by Request ID versioning.  
> Local completions read pre‑cached `buffer_words` + `vocab_words` + active‑line delta scan – zero rope I/O on each keystroke.  
> Ghost text only renders in Insert and Brief modes.

---

## ✨ Implemented Features

- ✅ **Vim‑like Modal Editing** – Normal/Insert/Visual/Command/Brief with smooth transitions.  
- ✅ **Brief Mode (Always‑Insert)** – All printable keys insert text, Alt shortcuts for commands (`Alt+s` save, `Alt+d` delete line, etc.).  
- ✅ **Split Window Layouts** – `:sp` / `:vs`, `Ctrl+w h/j/k/l` navigation, `:on` close others.  
- ✅ **Multi‑Buffer Editing** – Tab bar, `:e`, `:bn`/`:bp`, `:bd`, `:ls`.  
- ✅ **AI Ghost‑Text Completions (Codeium)** – Inline ghost with prefix‑overlap dedup, Tab/Right‑Arrow accept, Ctrl+N/P cycle.  
- ✅ **Local LLM & Conventional Commits** – `llama.cpp` integration, `:llm` chat, automatic commit generation (`:gc` / `c`).  
- ✅ **Tree‑sitter Syntax & Text Objects** – Highlighting, `diw`, `ci"`, `di(`, `ci{`, `di[`, `dif`.  
- ✅ **Vim‑style Search & Highlighting** – `/`, `n`/`N`, `*`, wrap‑around configurable.  
- ✅ **Leader Key & Mapleader Overrides** – Sequences like `space w`, configurable `leader`.  
- ✅ **Interactive Git Workspace** – `*git-status*` buffer with stage/unstage, commit generation, stash push/pop, safe branch switching.  
- ✅ **Git Gutter Diff** – Coloured signs, hybrid relative numbers, bookmarks.  
- ✅ **Vim Dot Repeat (`.`)** – Repeats normal mode changes, insert sessions, text object edits.  
- ✅ **Which‑Key Popup** – Bottom‑right hint for pending sequences.  
- ✅ **Local Autocomplete Fallback** – Merges vocab words, buffer words, active line delta.  
- ✅ **Register Copy‑Paste & Undo** – `yy`, `dd`, `space p v`, per‑buffer undo stack.  
- ✅ **File I/O with Auto‑Format** – `:w`, `rustfmt` on `.rs`, tab completion for paths.  
- ✅ **Reflection Config Dashboard** – `:config` – toggle booleans via Space, saves instantly.  
- ✅ **Persistent Command History** – `~/.config/ce/history.txt`, up/down navigation.  
- ✅ **Smart Cursor & Viewport Movement** – `desired_col` (curswant), PageUp/Down adapt to viewport.  
- ✅ **Dirty Buffer Quit Prompt** – Interactive `y`/`n`/`c` overlay.

---

## 🦀 Technology Stack

| Area            | Crates / Tools |
|----------------|----------------|
| Core Runtime    | Tokio (async, mpsc, intervals), Crossterm 0.29 (raw mode, press‑only events), Ratatui 0.29 |
| Text Buffer & Syntax | Ropey 1 (rope data structure), Tree‑sitter (incremental parsing, highlighting, text objects) |
| Networking      | reqwest 0.12 (rustls, json), sha2, flate2 (binary download & verification) |
| CLI & Config    | clap 4 (derive), serde / serde_json, dirs, uuid, anyhow |

---

## ⌨️ Command & Key Reference

### Normal Mode
- **Movement:** `h j k l` · `w b` · `0 $` · `gg` / `G` · `PageUp` / `PageDown` (viewport‑aware)  
- **Editing:** `x` · `dd` (delete line, yank) · `yy` (yank line) · `u` (undo) · `>` / `<` (indent/outdent)  
- **Text Objects:** `diw` / `ciw` · `di"` / `ci"` · `di(` / `ci(` · `di{` / `ci{` · `di[` / `ci[` · `dif` / `cif`  
- **Completions:** `Tab` / `Right` accept ghost (if visible) · `Ctrl+N` / `Ctrl+P` cycle  
- **Splits:** `Ctrl+w s` / `v` · `Ctrl+w h/j/k/l/w` · `Ctrl+w q`  
- **Global:** `space w` (save) · `space q` (quit) · `space Q` (force quit) · `space p v` (paste) · `space t` (toggle which‑key popup) · `Alt+d` (delete line)

### Insert Mode
- Direct input, `Backspace`, `Delete`, `Enter` (auto‑indent), `Tab` (soft‑tab 4 spaces or accept ghost)  
- `Right` accept ghost · `Home` / `End` · `Left` / `Right` · `Up` / `Down` (cycle completions if ghost active)  
- `Ctrl+N` / `Ctrl+P` cycle completions · `Alt+d` delete line · `Esc` → Normal (cursor left 1 col)

### Brief Mode (Always‑Insert)
- All printable keys insert text, `Backspace`, `Delete`, `Enter`, `Tab` (accept ghost or insert soft‑tab)  
- `Up` / `Down` cycle completions if ghost active, else move · `F9` open command line  
- `Esc` clear ghost/status · `Alt+x` / `Ctrl+c` → Normal · `Alt+s` (save) · `Alt+q` (quit) · `Alt+d` (delete line)  
- `Alt+u` (undo) · `Alt+y` (yank) · `Alt+p` (paste) · `Alt+b`/`f` (word back/forward) · `Alt+a`/`e` (line start/end)  
- `Alt+<`/`>` (first/last line) · `Alt+1-4` (switch buffer)

### Command Mode (`:`)
- **Buffer:** `:q` (dirty check) · `:q!` (force) · `:w` · `:w <path>` · `:wq` / `:x` · `:e <path>` · `:new` · `:ls` / `:buffers` · `:b N` · `:bn` / `:bp` · `:bd`  
- **Windows:** `:sp` / `:vs` · `:on`  
- **Modes:** `:vim` (Normal) · `:brief` (Brief)  
- **Config:** `:config` · `:vocab <word>`  
- **Git:** `:gs` / `:gitstatus` · `:stash [msg]`  
- **LLM:** `:prompt <msg>` / `:> <msg>` (chat query)  
- **History:** `Up` / `Down` navigate, `Tab` complete command/path, `Esc` cancel

### Git Status Buffer Shortcuts (active in `*git-status*`, Normal mode)
- `s` – Stage/unstage file under cursor  
- `c` – Auto‑stage tracked changes and generate LLM commit message  
- `z` – Pre‑fill `:stash ` in command line  
- `Enter` – Open file, switch branch, or pop stash (contextual)  
- `y` / `n` – Confirm/cancel interactive prompts (checkout, stash pop)  
- `q` / `Esc` – Exit status panel, restore previous mode

---

## 📋 TODO – Roadmap & Planned Features

### Core Editor
- [x] Multi‑buffer editing with tab bar  
- [x] Vim‑like modal editing  
- [x] Brief mode (Always‑Insert)  
- [x] Split window layouts  
- [x] Smart cursor vertical movement (desired_col)  
- [x] Undo stack (500 snapshots)  
- [x] File I/O with rustfmt auto‑format for `.rs`  
- [x] CJK‑aware char width  
- [x] Yank/Put (`yy`, `dd`, `space p v`)  
- [x] Persistent command history  
- [x] Interactive dirty‑buffer quit prompt  
- [x] Viewport‑aware PageUp/PageDown  
- [x] Dot repeat (`.`)  
- [ ] Redo (`Ctrl+R`) – **high priority**  
- [x] Visual mode (`v`, `V`)  
- [x] Search (`/ n N *`)  
- [ ] Replace (`:%s/old/new/g`) – medium priority  
- [ ] Count prefix (`3j`, `5dd`) – low priority  

### Syntax & Rendering
- [x] Tree‑sitter highlighting (Rust, Python, JS/TS)  
- [x] Text objects (`diw`, `ci"`, `di(`, `di{`, `di[`, `dif`)  
- [x] Scope extraction in status bar  
- [x] Block/bar cursor  
- [x] Clean ghost text (no inline `[1/8]`)  
- [x] Standard Vim bottom layout (command line at very bottom)  
- [x] Which‑Key and Config popups  
- [x] Dynamic Gutter (bookmarks, relative numbers, Git signs)  
- [x] Custom `gitstatus` buffer coloring  
- [ ] Theme / Color scheme support – medium priority  
- [ ] Indentation guides – low priority  

### Completion System
- [x] Ghost‑text inline display  
- [x] Tab / Right‑Arrow accept, Ctrl+N/P cycle  
- [x] 400 ms throttle + position dedup  
- [x] `CompletionProvider` async trait  
- [x] Top 5 candidates in status bar  
- [x] Local buffer autocomplete fallback (3‑source merge)  
- [x] Persistent wordlist via `:vocab`  
- [x] Works in Insert and Brief modes  
- [x] Fixed Codeium ghost stalling (`last_checked_pos` cleared)  
- [ ] Popup‑based completion dropdown – **high priority**  
- [ ] Fuzzy filtering of candidates – medium priority  

### Git & Workspace Safety
- [x] Interactive Git Status screen (`:gs`)  
- [x] Precise status parsing (fixed column shifts)  
- [x] Stage/Unstage/Untracked/Branch/Stash lists  
- [x] Non‑destructive branch switch (blocks if unclean)  
- [x] Automatic checkout reload, gutter sync, cursor clamp  
- [x] Git stash push with custom message  
- [x] Stash pop with confirmation prompt  
- [x] Prompt state‑machine (blocks key leaks)  
- [x] Git diff gutters & visual hunks overlay  
- [ ] Interactive staging tool – medium priority  

### Popup Menu System
- [x] `PopupState` / `PopupItem` / `PopupKind` types  
- [x] Floating centered overlay  
- [x] Config reflection dashboard (Space toggle)  
- [x] Which‑Key popup (bottom‑right, compact mods)  
- [ ] Command palette (`Ctrl+Shift+P`) – medium priority  
- [ ] Hover documentation (`K`) – medium priority  

### AI / Codeium & Llama.cpp Backends
- [x] Local LSP binary: find → download → spawn → port‑file poll  
- [x] gRPC‑JSON completions with request metadata  
- [x] Cloud REST fallback engine  
- [x] Smart auth (multi‑path key discovery + browser login)  
- [x] Request Versioning IDs  
- [x] Codeium disabled → no server spawn, no API key needed  
- [x] Fixed ghost text stalling when Codeium enabled  
- [x] Local `llama.cpp` raw TCP socket integration  
- [x] OpenAI‑compatible `/v1/chat/completions` payload  
- [x] Conventional git commit generation (`:gc` / `c`)  
- [x] Real‑time LLM chat session (`:llm` / `:prompt`)  
- [ ] Alternative backends: `ai::copilot`, `ai::local` (Ollama) – medium priority  

---

## 📄 License & Notes

**codeium-editor v0.1.0** – Modular architecture (11 modules) · ~17,000 LOC  
Built with Rust 2021, Tokio, Ratatui, Ropey, Tree‑sitter, Codeium AI LSP + Local Llama.cpp LLM backend.

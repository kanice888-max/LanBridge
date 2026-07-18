# LanBridge Frontend Redesign Guide

> Source: Figma `LanBridg`, redesign update document, LanBridge sync invariants.
> Purpose: help AI agents implement the redesigned frontend without misreading visual style, interaction states, or sync-safety rules.

| # | Section | What it captures |
|---|---|---|
| 1 | Visual Theme & Atmosphere | Mood, density, design philosophy |
| 2 | Color Palette & Roles | Semantic name + hex + functional role |
| 3 | Typography Rules | Font families, full hierarchy table |
| 4 | Component Stylings | Buttons, cards, inputs, navigation with states |
| 5 | Layout Principles | Spacing scale, grid, whitespace philosophy |
| 6 | Depth & Elevation | Shadow system, surface hierarchy |
| 7 | Do's and Don'ts | Design guardrails and anti-patterns |
| 8 | Responsive Behavior | Breakpoints, touch targets, collapsing strategy |
| 9 | Agent Prompt Guide | Quick color reference, ready-to-use prompts |

## 1. Visual Theme & Atmosphere

LanBridge should feel light, quiet, precise, and friendly. The interface uses a white-blue desktop utility aesthetic: soft dotted background, airy spacing, large animated folder as the central anchor, and pill-like controls.

The product is not a marketing page. It is a focused local desktop sync tool. Prioritize clarity, state visibility, and data safety over decoration.

Core mood:

- Clean white canvas with very pale blue atmosphere.
- Large blue folder illustration as the emotional and functional center.
- Rounded white controls with subtle shadows.
- Black primary pills for selected tabs and main actions.
- Small colored status marks for online, warning, error, success.

Do not describe LanBridge as fully bidirectional sync. It is Primary/Secondary sync with explicit return-sync.

## 2. Color Palette & Roles

Use semantic tokens rather than raw colors inside components.

| Token | Approx Hex | Role |
|---|---:|---|
| `--bg` | `#F8FBFF` | Main app background |
| `--bg-dot` | `#DDE8FA` | Low-opacity dotted texture |
| `--surface` | `#FFFFFF` | Cards, rows, popovers |
| `--surface-muted` | `#F3F4F6` | Input fill, disabled surfaces |
| `--text` | `#050505` | Primary text |
| `--text-muted` | `#9CA3AF` | Secondary metadata |
| `--text-soft-blue` | `#B7C7E8` | Section labels like "文件状态" |
| `--primary` | `#000000` | Active tab, primary CTA |
| `--primary-fg` | `#FFFFFF` | Text/icons on black controls |
| `--folder-blue` | `#1737FF` | Back folder body |
| `--folder-front` | `#93B9FA` | Front folder body |
| `--success` | `#5DDBB8` | Online dot, check mark |
| `--warning` | `#FFAA2B` | Pending/conflict attention |
| `--danger` | `#D83A3A` | Error, cancel, disconnected |
| `--accent-line` | `#1744FF` | Transfer progress bars |

Color rules:

- Black means active/primary, not destructive.
- Red means cancel, error, disconnected, or blocking issue.
- Orange means needs user decision, pending return, conflict, or warning.
- Green means online/success.
- Do not rely on color alone; pair status color with icon or text.

## 3. Typography Rules

Use system fonts for desktop-native feel.

| Role | Size | Weight | Usage |
|---|---:|---:|---|
| App nav | 14px | 500 | Top tabs |
| Page title | 26px | 500-600 | "项目名", "同步日志", "设置" |
| Section label | 12px | 400 | "文件状态", "历史记录" |
| Row primary | 15px | 500 | File name, device name |
| Row metadata | 12px | 400 | IP, size, timestamp |
| Button label | 13-16px | 500 | Pills and icon buttons |
| Toast title | 14px | 500 | Popover/toast title |
| Detail value | 14-16px | 400 | Paths, settings values |

Typography rules:

- Use Chinese UI copy as the default.
- Keep labels short and concrete.
- Avoid tiny unreadable text below 10px.
- Use tabular numbers for sizes, speeds, timestamps, and ports.
- Long file paths should ellipsize in rows and show full value in tooltip/popover.

## 4. Component Stylings

### Navigation

Top navigation contains logo plus `同步 / 发现 / 日志 / 设置`. Active item is a black rounded pill with white text. Inactive items are plain black text on transparent background.

Rules:

- Use Pill Nav motion for active tab transition.
- No separate global History tab; history is task-level inside sync page.
- Settings is a top-level tab, not a modal-only hidden action.

### Buttons

Primary CTA:

- Black pill, white text, large radius, height around 46-50px.
- Examples: `扫描并同步`, `发送邀请`, `回传到主机`.

Icon buttons:

- White circular buttons, subtle shadow.
- Used for open folder, delete, connection, info, history, refresh.
- Icon-only buttons must have accessible label/title.

Danger buttons:

- Red pill or red circular icon.
- Used for cancel invite, reject invite, disconnect, transfer cancel.

### Cards and Rows

Device cards:

- White rounded rectangle, subtle shadow.
- Device name bold, IP/port muted, online/offline dot on right.
- Discovery carousel supports mouse hover wheel scrolling and left-button drag scrolling.
- Carousel edges fade from 100% to 0% with optional progressive blur.

File rows:

- White rounded long cards.
- Left: file name.
- Right: size plus status icon.
- Status icons: green check for synced, orange warning for pending/conflict.

Settings/log rows:

- Full-width white rounded rows.
- Left content aligned to page grid, right metadata/value aligned to right.

### Inputs

Inputs use soft grey fill inside white surfaces. For folder path selection, use a wide rounded input plus black `选择文件夹` button on the right.

Folder selection rule:

- Target folder must be empty for task invite acceptance.
- If non-empty, show blocking error: `请选择一个空文件夹`.
- Do not allow "continue anyway".

### Popovers and Toasts

Popover style:

- White floating surface, radius 12-16px, soft shadow.
- Optional small triangular pointer toward trigger.
- Used for local address, network check, connection state, info/history panels.

Top toast:

- Centered near top.
- White card, red icon for blocking error.
- Used for non-empty folder and critical status.

## 5. Layout Principles

Base frame follows Figma desktop size `863 x 561`. Treat it as the canonical desktop composition.

Global layout:

- Header height around 64px.
- Logo at top-left.
- Main work area centered vertically with generous whitespace.
- Background dotted texture fills the entire viewport.
- Subtle ripple circles originate from lower center behind the folder.

Discovery page:

- Folder centered.
- Status text below folder.
- Device carousel at bottom.
- Top-right utility buttons: local address, manual input, network check.

Sync page:

- Left column: role badge, project name, large folder, primary action buttons.
- Right column: file status list.
- Top-right: connection status, info, history.
- Default task shown is the latest project when tasks exist.

Connection step flow:

- Stepper sits below folder.
- Completed/current steps are black and clickable when reachable.
- Future steps are grey and disabled.

Spacing:

- Use 8px grid.
- Common gaps: 8, 12, 16, 24, 32.
- Avoid dense dashboard panels; this design is stage-like and airy.

## 6. Depth & Elevation

Elevation should be soft and restrained.

| Level | Usage | Shadow |
|---|---|---|
| 0 | Background | none |
| 1 | File rows, settings rows | `0 8px 24px rgba(30, 64, 175, 0.06)` |
| 2 | Device cards, inputs | `0 12px 32px rgba(30, 64, 175, 0.08)` |
| 3 | Popovers, toasts | `0 18px 45px rgba(15, 23, 42, 0.10)` |
| 4 | Transfer stack overlays | `0 20px 50px rgba(15, 23, 42, 0.14)` |

Depth rules:

- Do not use heavy dark shadows.
- Use blur only for overlays, carousel edge fading, or background atmosphere.
- Transfer cards may overlap as a stack, then expand into individual cards.

## 7. Do's and Don'ts

Do:

- Preserve Primary/Secondary sync semantics.
- Keep pending return-sync explicit.
- Show waiting, rejected, disconnected, failed, conflict, and cancelled states.
- Back up Primary before confirmed overwrite.
- Keep the folder animation as the primary visual anchor.
- Reuse button, row, card, popover, input, toast, and status styles.

Don't:

- Do not silently overwrite or permanently delete synced user files.
- Do not call the product fully bidirectional sync.
- Do not let non-empty invite folders continue.
- Do not hide conflicts inside generic warnings.
- Do not add unrelated gradients, marketing hero sections, or decorative cards.
- Do not upgrade React/Tauri major versions just for the redesign.
- Do not import entire animation/UI repositories if only one small pattern is needed.

## 8. Responsive Behavior

Primary target is desktop Tauri.

Breakpoints:

- `>= 860px`: use Figma two-column sync layout.
- `700-859px`: reduce side margins and file row width.
- `< 700px`: stack project area above file list; keep top navigation horizontal if it fits, otherwise reduce gaps.

Targets:

- Minimum interactive hit area: 36px desktop, 44px touch-capable.
- Icon buttons remain square/circular and do not shrink below usable size.
- Long labels wrap only when necessary; prefer shorter Chinese labels.

Motion:

- Use `motion` for page transitions, list entry, popovers, stepper, tab pill, transfer stack.
- Durations: 150-300ms for normal UI, up to 400ms for folder animation.
- Animate transform and opacity, not layout-heavy properties.
- Respect `prefers-reduced-motion`.

## 9. Agent Prompt Guide

Quick implementation prompt:

"Implement the LanBridge redesign using the Figma visual language: pale dotted white-blue background, black pill navigation, large animated blue folder, white rounded rows, soft shadows, explicit sync safety states, and concise Chinese UI copy. Preserve Primary/Secondary sync semantics and never allow non-empty invite folders to continue."

Quick color reference:

- Background `#F8FBFF`
- Surface `#FFFFFF`
- Text `#050505`
- Muted text `#9CA3AF`
- Primary black `#000000`
- Folder blue `#1737FF`
- Folder front `#93B9FA`
- Success `#5DDBB8`
- Warning `#FFAA2B`
- Danger `#D83A3A`

Implementation defaults:

- Work in `worktrees/macos` first.
- Keep React 18, Vite 6, Tauri 1.6.
- Add only minimal dependencies: `motion` plus necessary Radix primitives.
- Treat React Bits, Magic UI, Motion Primitives, and Animate UI as reference sources unless a component is explicitly copied/adapted.
- Validate with `npm run lint:names`, `npm run build`, and `cargo test --manifest-path src-tauri/Cargo.toml`.

## Assumptions

- Save path is `redesign/design.md`.
- Body text uses Chinese-facing product guidance where needed while keeping English section names for AI and engineering context.
- Color values are approximate tokens inferred from the Figma screenshots; implementation may fine-tune them after extracting exact Figma values.

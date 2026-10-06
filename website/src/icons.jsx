/*
 * cokacmux website icons.
 *
 * Every glyph in this file was drawn by hand in this repository for the
 * cokacmux tutorial site, directly in code from basic SVG primitives
 * (rect, circle, line, polyline, polygon and simple arc/curve paths) on a
 * 24x24 grid. No external icon set, logo, font or path data was copied,
 * traced or adapted.
 *
 * Production notes (origin record, see docs/MATERIALS_PROVENANCE.md):
 * - Author: written in code for this repo, 2026-10-06, replacing the former
 *   third-party icon package dependency.
 * - Style: 2px round stroke in `currentColor`, so the existing CSS colors
 *   (`.valueCard svg`, `.heroFacts svg`, ...) keep applying.
 * - The source-code link icon is a generic "< / >" symbol, not a
 *   third-party brand mark.
 */

function IconBase({ size = 24, children, ...props }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...props}
    >
      {children}
    </svg>
  );
}

// Eight-point rosette seal (16-vertex polygon, radii 9.5 / 7.9) with a check mark.
export function CheckBadgeIcon(props) {
  return (
    <IconBase {...props}>
      <polygon points="12 2.5 15.02 4.7 18.72 5.28 19.3 8.98 21.5 12 19.3 15.02 18.72 18.72 15.02 19.3 12 21.5 8.98 19.3 5.28 18.72 4.7 15.02 2.5 12 4.7 8.98 5.28 5.28 8.98 4.7" />
      <polyline points="8.6 12.2 11 14.5 15.6 9.7" />
    </IconBase>
  );
}

// Clipboard board with a rounded clip tab sitting on its top edge and three bullet rows.
export function ClipboardListIcon(props) {
  return (
    <IconBase {...props}>
      <rect x="5" y="4.5" width="14" height="17" rx="2" />
      <path d="M9 4.5V3.5A1.5 1.5 0 0 1 10.5 2h3A1.5 1.5 0 0 1 15 3.5v1" />
      <path d="M8.5 10.5h.01M8.5 14h.01M8.5 17.5h.01" />
      <path d="M11.5 10.5h4.5M11.5 14h4.5M11.5 17.5h3" />
    </IconBase>
  );
}

// Back page peeking out behind a front page that carries two text lines.
export function CopyIcon(props) {
  return (
    <IconBase {...props}>
      <path d="M8.5 17H6a1.5 1.5 0 0 1-1.5-1.5V5A1.5 1.5 0 0 1 6 3.5h8.5A1.5 1.5 0 0 1 16 5v2.5" />
      <rect x="8.5" y="7.5" width="11.5" height="13.5" rx="1.5" />
      <path d="M11.5 12H17M11.5 15.5h4" />
    </IconBase>
  );
}

// Tabbed folder outline that stops short of a magnifier in its lower-right corner.
export function FolderSearchIcon(props) {
  return (
    <IconBase {...props}>
      <path d="M11 19.5H4.5A1.5 1.5 0 0 1 3 18V5.5A1.5 1.5 0 0 1 4.5 4H9l2 2.5h8.5A1.5 1.5 0 0 1 21 8v3" />
      <circle cx="16.5" cy="15.5" r="3" />
      <line x1="18.7" y1="17.7" x2="21" y2="20" />
    </IconBase>
  );
}

// Generic source-code symbol "< / >" used for the repository link (no brand mark).
export function SourceCodeIcon(props) {
  return (
    <IconBase {...props}>
      <polyline points="7.5 7 2.5 12 7.5 17" />
      <polyline points="16.5 7 21.5 12 16.5 17" />
      <line x1="13.5" y1="5" x2="10.5" y2="19" />
    </IconBase>
  );
}

// 315-degree clock arc with a downward arrowhead at 9 o'clock (rewind) and two hands.
export function HistoryIcon(props) {
  return (
    <IconBase {...props}>
      <path d="M4 12A8.5 8.5 0 1 1 6.49 18.01" />
      <polyline points="1.8 9.8 4 12 6.2 9.8" />
      <polyline points="12.5 7.5 12.5 12 16 12" />
    </IconBase>
  );
}

// Wide key deck: two staggered rows of key dots and a space bar.
export function KeyboardIcon(props) {
  return (
    <IconBase {...props}>
      <rect x="2.5" y="6" width="19" height="12" rx="2" />
      <path d="M6 9.5h.01M9 9.5h.01M12 9.5h.01M15 9.5h.01M18 9.5h.01" />
      <path d="M7.5 12.5h.01M10.5 12.5h.01M13.5 12.5h.01M16.5 12.5h.01" />
      <line x1="8" y1="15.5" x2="16" y2="15.5" />
    </IconBase>
  );
}

// Diamond plate with two chevrons below it, offset by 4.5 units each.
export function LayersIcon(props) {
  return (
    <IconBase {...props}>
      <polygon points="12 3 21 7.5 12 12 3 7.5" />
      <polyline points="3 12 12 16.5 21 12" />
      <polyline points="3 16.5 12 21 21 16.5" />
    </IconBase>
  );
}

// Right-pointing triangle with rounded joins.
export function PlayIcon(props) {
  return (
    <IconBase {...props}>
      <polygon points="8 5 19 12 8 19" />
    </IconBase>
  );
}

// Lens circle with a small glint arc and a diagonal handle.
export function SearchIcon(props) {
  return (
    <IconBase {...props}>
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="M8 10.5A2.5 2.5 0 0 1 10.5 8" />
      <line x1="15.2" y1="15.2" x2="20.5" y2="20.5" />
    </IconBase>
  );
}

// Flat-topped shield built from lines and two curves, with an exclamation mark.
export function ShieldAlertIcon(props) {
  return (
    <IconBase {...props}>
      <path d="M12 2.75L19.5 5.5V11C19.5 15.8 16.3 19.4 12 21.25C7.7 19.4 4.5 15.8 4.5 11V5.5Z" />
      <line x1="12" y1="7.5" x2="12" y2="12.5" />
      <path d="M12 15.75h.01" />
    </IconBase>
  );
}

// Two side-by-side pane windows, each with a title-bar line.
export function SplitPanesIcon(props) {
  return (
    <IconBase {...props}>
      <rect x="3" y="4.5" width="8" height="15" rx="1.5" />
      <rect x="13" y="4.5" width="8" height="15" rx="1.5" />
      <path d="M3 8h8M13 8h8" />
    </IconBase>
  );
}

// Terminal window with a ">" prompt chevron and an underscore cursor.
export function TerminalIcon(props) {
  return (
    <IconBase {...props}>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <polyline points="7 9.5 10 12 7 14.5" />
      <line x1="12.5" y1="15" x2="16.5" y2="15" />
    </IconBase>
  );
}

// Diagonal wand stick, a pinched four-point star at its tip, plus a small sparkle and dot.
export function WandIcon(props) {
  return (
    <IconBase {...props}>
      <line x1="3.5" y1="20.5" x2="12.5" y2="11.5" />
      <path d="M17 3.5Q17 7 20.5 7Q17 7 17 10.5Q17 7 13.5 7Q17 7 17 3.5Z" />
      <path d="M7 3.5v3M5.5 5h3" />
      <path d="M20 15.5h.01" />
    </IconBase>
  );
}

import changelogRaw from "../CHANGELOG.md?raw";
import type { Locale } from "./types";

export type InlineLeafSegment =
  | { type: "text"; value: string }
  | { type: "code"; value: string };

export type InlineSegment =
  | InlineLeafSegment
  | { type: "strong"; children: InlineLeafSegment[] };

export interface ReleaseNoteBullet {
  text: string;
  subItems: string[];
}

export interface ReleaseEntry {
  version: string;
  date: string | null;
  titleJa: string;
  titleEn: string;
  itemsJa: ReleaseNoteBullet[];
  itemsEn: ReleaseNoteBullet[];
}

const CJK_RE = /[\u3000-\u30ff\u4e00-\u9fff\uff00-\uffef]/;

export function normalizeReleaseDate(
  raw: string | null | undefined
): string | null {
  if (!raw) return null;
  const trimmed = raw.trim();
  const isoMatch = trimmed.match(/^(\d{4}-\d{2}-\d{2})/);
  return isoMatch ? isoMatch[1] : null;
}

export function splitBilingualSubtitle(subtitle: string): {
  titleJa: string;
  titleEn: string;
} {
  const trimmed = subtitle.trim();
  if (!trimmed) {
    return { titleJa: "", titleEn: "" };
  }

  const separator = " / ";
  const splitPositions: number[] = [];
  let searchFrom = 0;
  while (searchFrom < trimmed.length) {
    const idx = trimmed.indexOf(separator, searchFrom);
    if (idx === -1) break;
    splitPositions.push(idx);
    searchFrom = idx + separator.length;
  }

  if (splitPositions.length === 0) {
    return { titleJa: trimmed, titleEn: trimmed };
  }

  for (const idx of splitPositions) {
    const left = trimmed.slice(0, idx).trim();
    const right = trimmed.slice(idx + separator.length).trim();
    if (left && right && CJK_RE.test(left) && !CJK_RE.test(right)) {
      return { titleJa: left, titleEn: right };
    }
  }

  const lastIdx = splitPositions[splitPositions.length - 1];
  const fallbackLeft = trimmed.slice(0, lastIdx).trim();
  const fallbackRight = trimmed.slice(lastIdx + separator.length).trim();
  return {
    titleJa: fallbackLeft || trimmed,
    titleEn: fallbackRight || trimmed,
  };
}

function parseBullets(lines: string[]): ReleaseNoteBullet[] {
  const bullets: ReleaseNoteBullet[] = [];

  for (const rawLine of lines) {
    const trimmed = rawLine.trim();
    if (
      !trimmed ||
      trimmed === "---" ||
      trimmed.startsWith(">") ||
      trimmed.startsWith("#")
    ) {
      continue;
    }

    const subMatch = rawLine.match(/^\s{2,}[*-]\s+(.+)$/);
    if (subMatch) {
      const subText = subMatch[1].trim();
      if (bullets.length > 0) {
        bullets[bullets.length - 1].subItems.push(subText);
      } else {
        bullets.push({ text: subText, subItems: [] });
      }
      continue;
    }

    const topMatch = rawLine.match(/^\s{0,1}[*-]\s+(.+)$/);
    if (topMatch) {
      bullets.push({ text: topMatch[1].trim(), subItems: [] });
      continue;
    }

    if (bullets.length > 0) {
      const last = bullets[bullets.length - 1];
      if (last.subItems.length > 0) {
        last.subItems[last.subItems.length - 1] += ` ${trimmed}`;
      } else {
        last.text += ` ${trimmed}`;
      }
    }
  }

  return bullets;
}

function parseSectionSubsections(sectionLines: string[]): {
  itemsJa: ReleaseNoteBullet[];
  itemsEn: ReleaseNoteBullet[];
} {
  const jaLines: string[] = [];
  const enLines: string[] = [];
  const unheadedLines: string[] = [];
  let mode: "none" | "ja" | "en" | "other" = "none";

  for (const line of sectionLines) {
    if (/^###\s+主な変更点\s*$/.test(line)) {
      mode = "ja";
      continue;
    }
    if (/^###\s+What's Changed\s*$/i.test(line)) {
      mode = "en";
      continue;
    }
    if (/^###\s+/.test(line)) {
      mode = "other";
      continue;
    }

    if (mode === "ja") {
      jaLines.push(line);
    } else if (mode === "en") {
      enLines.push(line);
    } else if (mode === "none") {
      unheadedLines.push(line);
    }
  }

  let itemsJa = parseBullets(jaLines);
  let itemsEn = parseBullets(enLines);
  const fallbackItems = parseBullets(unheadedLines);

  if (itemsJa.length === 0 && itemsEn.length === 0 && fallbackItems.length > 0) {
    itemsJa = fallbackItems;
    itemsEn = fallbackItems;
  } else if (itemsJa.length === 0) {
    itemsJa = itemsEn;
  } else if (itemsEn.length === 0) {
    itemsEn = itemsJa;
  }

  return { itemsJa, itemsEn };
}

export function parseChangelogMarkdown(raw: string): ReleaseEntry[] {
  const lines = raw.replace(/\r\n/g, "\n").split("\n");
  const headerRe =
    /^##\s+\[([^\]]+)\](?:\s*\(([^)]*)\))?(?:\s*(?:—|--|-)\s*(.+))?\s*$/;

  const entries: ReleaseEntry[] = [];
  let currentHeader: {
    version: string;
    date: string | null;
    subtitle: string;
  } | null = null;
  let currentLines: string[] = [];

  const flushCurrent = () => {
    if (!currentHeader) return;
    const { titleJa, titleEn } = splitBilingualSubtitle(currentHeader.subtitle);
    const { itemsJa, itemsEn } = parseSectionSubsections(currentLines);
    if (itemsJa.length > 0 || itemsEn.length > 0) {
      entries.push({
        version: currentHeader.version,
        date: currentHeader.date,
        titleJa,
        titleEn,
        itemsJa,
        itemsEn,
      });
    }
    currentHeader = null;
    currentLines = [];
  };

  for (const line of lines) {
    const m = line.match(headerRe);
    if (m) {
      flushCurrent();
      currentHeader = {
        version: m[1].trim(),
        date: normalizeReleaseDate(m[2]),
        subtitle: (m[3] ?? "").trim(),
      };
      continue;
    }
    if (currentHeader) {
      currentLines.push(line);
    }
  }
  flushCurrent();

  return entries;
}

export function parseSingleUpdateNotes(
  version: string,
  date: string | null | undefined,
  rawNotes: string | null | undefined
): ReleaseEntry | null {
  if (!rawNotes) return null;
  const lines = rawNotes.replace(/\r\n/g, "\n").trim().split("\n");
  if (lines.length === 0) return null;

  let subtitle = "";
  let bodyLines = lines;

  const firstLineMatch = lines[0]
    .trim()
    .match(/^(?:##\s*)?Shizuka\s+v?\S+(?:\s*(?:—|--|-)\s*(.+))?$/i);
  if (firstLineMatch) {
    subtitle = (firstLineMatch[1] ?? "").trim();
    bodyLines = lines.slice(1);
  }

  const { titleJa, titleEn } = splitBilingualSubtitle(subtitle);
  const { itemsJa, itemsEn } = parseSectionSubsections(bodyLines);
  if (itemsJa.length === 0 && itemsEn.length === 0) {
    return null;
  }

  return {
    version: version.replace(/^v/, "").trim(),
    date: normalizeReleaseDate(date),
    titleJa,
    titleEn,
    itemsJa,
    itemsEn,
  };
}

let cachedChangelog: ReleaseEntry[] | null = null;

export function getBundledChangelog(): ReleaseEntry[] {
  if (cachedChangelog === null) {
    cachedChangelog = parseChangelogMarkdown(changelogRaw);
  }
  return cachedChangelog;
}

export function getReleaseTitle(entry: ReleaseEntry, locale: Locale): string {
  return locale === "ja"
    ? entry.titleJa || entry.titleEn
    : entry.titleEn || entry.titleJa;
}

export function getReleaseItems(
  entry: ReleaseEntry,
  locale: Locale
): ReleaseNoteBullet[] {
  return locale === "ja"
    ? entry.itemsJa.length > 0
      ? entry.itemsJa
      : entry.itemsEn
    : entry.itemsEn.length > 0
    ? entry.itemsEn
    : entry.itemsJa;
}

function tokenizePlainAndCode(input: string): InlineLeafSegment[] {
  if (!input) return [];
  const tokens: InlineLeafSegment[] = [];
  const tokenRe = /`([^`]+)`|\[([^\]]+)\]\(([^)]+)\)/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = tokenRe.exec(input)) !== null) {
    if (match.index > lastIndex) {
      tokens.push({
        type: "text",
        value: input.slice(lastIndex, match.index),
      });
    }

    if (match[1] !== undefined) {
      tokens.push({ type: "code", value: match[1] });
    } else if (match[2] !== undefined) {
      const rawLabel = match[2].trim();
      const cleanLabel = rawLabel.replace(/^`+|`+$/g, "");
      const looksLikeCodeOrFile =
        rawLabel.startsWith("`") || /\.[a-z0-9]{1,6}$/i.test(cleanLabel);
      tokens.push({
        type: looksLikeCodeOrFile ? "code" : "text",
        value: cleanLabel,
      });
    }

    lastIndex = tokenRe.lastIndex;
  }

  if (lastIndex < input.length) {
    tokens.push({ type: "text", value: input.slice(lastIndex) });
  }

  return tokens;
}

export function tokenizeInlineMarkdown(text: string): InlineSegment[] {
  if (!text) return [];
  const segments: InlineSegment[] = [];
  const boldRe = /\*\*(.+?)\*\*/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = boldRe.exec(text)) !== null) {
    if (match.index > lastIndex) {
      segments.push(...tokenizePlainAndCode(text.slice(lastIndex, match.index)));
    }
    segments.push({
      type: "strong",
      children: tokenizePlainAndCode(match[1]),
    });
    lastIndex = boldRe.lastIndex;
  }

  if (lastIndex < text.length) {
    segments.push(...tokenizePlainAndCode(text.slice(lastIndex)));
  }

  return segments;
}

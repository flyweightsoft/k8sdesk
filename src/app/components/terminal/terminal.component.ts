import { CommonModule } from '@angular/common';
import {
  AfterViewChecked,
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  EventEmitter,
  Input,
  Output,
  ViewChild,
  computed,
  inject,
  signal,
} from '@angular/core';
import { FormsModule } from '@angular/forms';
import { DomSanitizer, SafeHtml } from '@angular/platform-browser';
import { HistoryEntry } from '../../app.component';

const COMMANDS = ['get', 'describe', 'logs', 'delete', 'scale', 'rollout', 'apply', 'help'];
const RESOURCES = [
  'pods', 'po', 'deployments', 'deploy', 'services', 'svc', 'nodes', 'no', 'namespaces', 'ns',
  'configmaps', 'cm', 'secrets', 'events', 'ev', 'replicasets', 'rs', 'statefulsets', 'sts',
  'daemonsets', 'ds', 'ingresses', 'ing', 'jobs', 'cronjobs', 'cj'
];

const STATUS_CLASS: Record<string, string> = {
  running: 'badge-running',
  succeeded: 'badge-ok',
  completed: 'badge-ok',
  pending: 'badge-pending',
  waiting: 'badge-pending',
  containercreating: 'badge-pending',
  podscheduled: 'badge-pending',
  initialized: 'badge-pending',
  failed: 'badge-error',
  error: 'badge-error',
  crashloopbackoff: 'badge-error',
  oomkilled: 'badge-error',
  imagepullbackoff: 'badge-error',
  errimagepull: 'badge-error',
  terminating: 'badge-warn',
  unknown: 'badge-warn',
  evicted: 'badge-warn',
};

const STATUS_HEADERS = new Set(['phase', 'status', 'ready', 'state', 'conditions']);

interface EntryGroup {
  /** Index of the first entry in the flat history array. Used as the clear key. */
  startIdx: number;
  input: (HistoryEntry & { kind: 'input' }) | null;
  followups: { idx: number; entry: HistoryEntry }[];
}

@Component({
  selector: 'app-terminal',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [CommonModule, FormsModule],
  templateUrl: './terminal.component.html',
  styleUrl: './terminal.component.scss',
})
export class TerminalComponent implements AfterViewChecked {
  @Input({ required: true }) history: HistoryEntry[] = [];
  @Input() disabled = false;
  @Output() run = new EventEmitter<{ command: string; applyBody?: string }>();
  @Output() clearAll = new EventEmitter<void>();
  @Output() clearGroup = new EventEmitter<number>();

  @ViewChild('scroll') private scroll?: ElementRef<HTMLElement>;

  private sanitizer = inject(DomSanitizer);

  input = signal('');
  applyOpen = signal(false);
  applyBody = signal('');
  applyId = signal('');
  copiedIdx = signal(-1);
  confirmClearAll = signal(false);
  confirmClearGroupIdx = signal<number | null>(null);
  cmdHistory: string[] = [];
  histIdx = -1;

  /** Index of the currently highlighted autocomplete suggestion. */
  acIdx = signal(0);

  /** The base text before the completing token (e.g. "get "). */
  private acBase = computed(() => {
    const cur = this.input();
    const lastSpace = cur.lastIndexOf(' ');
    return lastSpace === -1 ? '' : cur.substring(0, lastSpace + 1);
  });

  /** All matching completions for the current input. */
  acMatches = computed(() => {
    const cur = this.input();
    if (!cur) return [] as string[];
    const parts = cur.trimStart().split(/\s+/);
    const lastSpace = cur.lastIndexOf(' ');
    const prefix = lastSpace === -1 ? cur : cur.substring(lastSpace + 1);
    if (!prefix) return [] as string[];
    if (parts.length === 1) {
      return COMMANDS.filter(c => c.startsWith(prefix.toLowerCase()));
    }
    if (parts.length === 2 && ['get', 'describe', 'delete'].includes(parts[0].toLowerCase())) {
      return RESOURCES.filter(r => r.startsWith(prefix.toLowerCase()));
    }
    return [] as string[];
  });

  /** The untyped suffix of the current suggestion — drives the ghost overlay. */
  ghostRest = computed(() => {
    const matches = this.acMatches();
    if (!matches.length) return '';
    const idx = Math.min(this.acIdx(), matches.length - 1);
    const full = this.acBase() + matches[idx];
    const cur  = this.input();
    if (full.length > cur.length && full.toLowerCase().startsWith(cur.toLowerCase())) {
      return full.slice(cur.length);
    }
    return '';
  });

  ngAfterViewChecked(): void {
    if (this.scroll) {
      const el = this.scroll.nativeElement;
      el.scrollTop = el.scrollHeight;
    }
  }

  submit(): void {
    let cmd = this.input().trim();
    if (!cmd || this.disabled) return;
    this.cmdHistory.push(cmd);
    this.histIdx = this.cmdHistory.length;
    this.input.set('');

    // Allow users to optionally prefix commands with `kubectl` (the cluster
    // and namespace are managed by the app, so the prefix is just stripped).
    if (/^kubectl\s+/i.test(cmd)) {
      cmd = cmd.replace(/^kubectl\s+/i, '');
    }

    if (cmd.startsWith('apply ')) {
      // Open the paste dialog; tag with current command's paste id.
      const tok = cmd.split(/\s+/)[1];
      const id = tok?.startsWith('paste:') ? tok.slice('paste:'.length) : 'inline';
      this.applyId.set(id);
      this.applyBody.set('');
      this.applyOpen.set(true);
      // Save the command text for after the paste is supplied.
      this.pendingApply = `apply paste:${id}`;
      return;
    }

    this.run.emit({ command: cmd });
  }

  private pendingApply: string | null = null;

  submitApply(): void {
    const body = this.applyBody().trim();
    if (!body || !this.pendingApply) return;
    const cmd = this.pendingApply;
    this.pendingApply = null;
    this.applyOpen.set(false);
    this.run.emit({ command: cmd, applyBody: body });
  }

  cancelApply(): void {
    this.applyOpen.set(false);
    this.pendingApply = null;
  }

  requestClearAll(): void {
    this.confirmClearAll.set(true);
  }

  confirmClearAllNow(): void {
    this.confirmClearAll.set(false);
    this.clearAll.emit();
  }

  cancelClearAll(): void {
    this.confirmClearAll.set(false);
  }

  requestClearGroup(idx: number): void {
    this.confirmClearGroupIdx.set(idx);
  }

  confirmClearGroupNow(): void {
    const idx = this.confirmClearGroupIdx();
    this.confirmClearGroupIdx.set(null);
    if (idx !== null) this.clearGroup.emit(idx);
  }

  cancelClearGroup(): void {
    this.confirmClearGroupIdx.set(null);
  }

  /** Groups flat history entries into [input + its followups] bundles. */
  get groups(): EntryGroup[] {
    const result: EntryGroup[] = [];
    const h = this.history;
    let i = 0;
    while (i < h.length) {
      const e = h[i];
      if (e.kind === 'input') {
        const group: EntryGroup = { startIdx: i, input: e, followups: [] };
        i++;
        while (i < h.length && h[i].kind !== 'input') {
          group.followups.push({ idx: i, entry: h[i] });
          i++;
        }
        result.push(group);
      } else {
        // Standalone error (e.g. namespace load failure — no preceding input)
        result.push({ startIdx: i, input: null, followups: [{ idx: i, entry: e }] });
        i++;
      }
    }
    return result;
  }

  statusClass(val: string, header: string): string {
    if (!STATUS_HEADERS.has(header.toLowerCase())) return '';
    const key = val.trim().toLowerCase().replace(/[^a-z]/g, '');
    return STATUS_CLASS[key] ?? '';
  }

  isStatusHeader(header: string): boolean {
    return STATUS_HEADERS.has(header.toLowerCase());
  }

  resourceIcon(firstHeader: string): string {
    const h = firstHeader.toLowerCase();
    if (h === 'name') return '◈';
    return '▸';
  }

  yamlHtml(body: string): SafeHtml {
    const html = body
      .split('\n')
      .map(line => this.highlightYamlLine(line))
      .join('\n');
    return this.sanitizer.bypassSecurityTrustHtml(html);
  }

  private esc(s: string): string {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  private highlightYamlLine(line: string): string {
    const trimmed = line.trimStart();
    const indent = this.esc(line.substring(0, line.length - trimmed.length));
    if (!trimmed) return '';
    if (trimmed.startsWith('#')) {
      return `${indent}<span class="yc">${this.esc(trimmed)}</span>`;
    }
    const colonIdx = trimmed.indexOf(':');
    if (colonIdx > 0) {
      const key = trimmed.substring(0, colonIdx);
      const rest = trimmed.substring(colonIdx + 1);
      const keySpan = `<span class="yk">${this.esc(key)}</span><span class="yp">:</span>`;
      if (!rest.trim()) return indent + keySpan;
      return indent + keySpan + this.colorizeYamlValue(rest);
    }
    if (trimmed.startsWith('- ')) {
      return indent + `<span class="yp">- </span>` + this.colorizeYamlValue(trimmed.substring(2));
    }
    return this.esc(line);
  }

  private colorizeYamlValue(val: string): string {
    const t = val.trim();
    if (t === 'null' || t === '~') return ` <span class="yn">${this.esc(val)}</span>`;
    if (t === 'true' || t === 'false') return ` <span class="yb">${this.esc(val)}</span>`;
    if (/^-?\d+(\.\d+)?$/.test(t)) return ` <span class="ynm">${this.esc(val)}</span>`;
    if ((t.startsWith('"') && t.endsWith('"')) || (t.startsWith("'") && t.endsWith("'"))) {
      return ` <span class="ys">${this.esc(val)}</span>`;
    }
    return ` <span class="yv">${this.esc(val)}</span>`;
  }

  copy(text: string, idx: number): void {
    navigator.clipboard.writeText(text).then(() => {
      this.copiedIdx.set(idx);
      setTimeout(() => this.copiedIdx.set(-1), 1500);
    });
  }

  tableText(headers: string[], rows: string[][]): string {
    const widths = headers.map((h, i) =>
      Math.max(h.length, ...rows.map((r) => (r[i] ?? '').length)),
    );
    const fmt = (cells: string[]) =>
      cells.map((c, i) => (c ?? '').padEnd(widths[i] ?? 0)).join('  ');
    return [fmt(headers), ...rows.map(fmt)].join('\n');
  }

  onKey(e: KeyboardEvent): void {
    // Typing a character resets the suggestion cycle back to the first match.
    if (e.key.length === 1 || e.key === 'Backspace' || e.key === 'Delete') {
      this.acIdx.set(0);
    }

    if (e.key === 'Tab') {
      e.preventDefault();
      const matches = this.acMatches();
      if (matches.length) {
        const idx = Math.min(this.acIdx(), matches.length - 1);
        this.input.set(this.acBase() + matches[idx] + ' ');
        this.acIdx.set(0);
      }
      return;
    }

    const suggestions = this.acMatches();

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (suggestions.length) {
        this.acIdx.set((this.acIdx() + 1) % suggestions.length);
      } else {
        // Fall back to command history
        if (this.histIdx < this.cmdHistory.length - 1) {
          this.histIdx++;
          this.input.set(this.cmdHistory[this.histIdx] ?? '');
        } else {
          this.histIdx = this.cmdHistory.length;
          this.input.set('');
        }
      }
      return;
    }

    if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (suggestions.length) {
        this.acIdx.set((this.acIdx() - 1 + suggestions.length) % suggestions.length);
      } else {
        // Fall back to command history
        if (this.histIdx > 0) {
          this.histIdx--;
          this.input.set(this.cmdHistory[this.histIdx] ?? '');
        }
      }
      return;
    }
  }
}

import {
  Component,
  Input,
  Output,
  EventEmitter,
  OnChanges,
  SimpleChanges,
  HostListener,
  computed,
  signal,
  inject,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { MonacoEditorModule } from 'ngx-monaco-editor-v2';
import { TauriService, DirEntry } from '../../services/tauri.service';
import { ThemeService } from '../../services/theme.service';

export interface FileTab {
  path: string;
  name: string;
  content: string;
  originalContent: string;
  dirty: boolean;
}

interface FlatNode {
  entry: DirEntry;
  depth: number;
}

@Component({
  selector: 'app-file-manager',
  standalone: true,
  imports: [CommonModule, FormsModule, MonacoEditorModule],
  templateUrl: './file-manager.component.html',
  styleUrl: './file-manager.component.scss',
})
export class FileManagerComponent implements OnChanges {
  @Input() clusterId: string | null = null;
  @Output() applyFile = new EventEmitter<{ path: string; content: string }>();

  private tauri = inject(TauriService);
  private themeService = inject(ThemeService);

  rootPath = signal<string | null>(null);
  tree = signal<DirEntry[]>([]);
  openTabs = signal<FileTab[]>([]);
  activeTabPath = signal<string | null>(null);
  expandedPaths = signal<Set<string>>(new Set());
  loading = signal(false);
  errorMsg = signal<string | null>(null);

  /** Monaco editor options. Theme tracks the app theme (dark vs light). */
  editorOptions = computed(() => ({
    language: 'yaml',
    theme: this.themeService.current().mode === 'light' ? 'vs' : 'vs-dark',
    minimap: { enabled: false },
    fontSize: 12,
    automaticLayout: true,
    scrollBeyondLastLine: false,
    wordWrap: 'on' as const,
  }));

  /** Flat list of visible tree nodes for rendering. */
  flatNodes = computed<FlatNode[]>(() => {
    const expanded = this.expandedPaths();
    const flatten = (nodes: DirEntry[], depth: number): FlatNode[] => {
      const result: FlatNode[] = [];
      for (const entry of nodes) {
        result.push({ entry, depth });
        if (entry.isDir && expanded.has(entry.path) && entry.children) {
          result.push(...flatten(entry.children, depth + 1));
        }
      }
      return result;
    };
    return flatten(this.tree(), 0);
  });

  activeTab = computed(() => {
    const path = this.activeTabPath();
    return this.openTabs().find(t => t.path === path) ?? null;
  });

  rootName = computed(() => {
    const p = this.rootPath();
    if (!p) return 'Files';
    const parts = p.replace(/\\/g, '/').split('/');
    return parts[parts.length - 1] || p;
  });

  /** Two-way binding for Monaco editor. Reading returns active tab content;
   *  writing updates the active tab and marks it dirty. */
  get editorContent(): string {
    return this.activeTab()?.content ?? '';
  }

  set editorContent(v: string) {
    const path = this.activeTabPath();
    if (!path) return;
    this.openTabs.update(tabs =>
      tabs.map(t => t.path === path
        ? { ...t, content: v, dirty: v !== t.originalContent }
        : t)
    );
  }

  async ngOnChanges(changes: SimpleChanges): Promise<void> {
    if (!('clusterId' in changes)) return;

    // Guard: if any open tab is dirty, confirm before switching
    const dirtyTabs = this.openTabs().filter(t => t.dirty);
    if (dirtyTabs.length) {
      const names = dirtyTabs.map(t => t.name).join(', ');
      const ok = await this.tauri.confirm(
        `You have unsaved changes in: ${names}\nSwitch cluster and discard changes?`
      );
      if (!ok) return;
    }

    // Reset state when cluster changes
    this.openTabs.set([]);
    this.activeTabPath.set(null);
    this.tree.set([]);
    this.rootPath.set(null);
    this.expandedPaths.set(new Set());
    this.errorMsg.set(null);

    const id = this.clusterId;
    if (!id) return;
    try {
      const folder = await this.tauri.clusterFolderGet(id);
      if (folder) await this.loadTree(folder);
    } catch (e) {
      this.errorMsg.set(String(e));
    }
  }

  async assignFolder(): Promise<void> {
    const picked = await this.tauri.pickFolder();
    if (!picked || !this.clusterId) return;
    try {
      await this.tauri.clusterFolderSet(this.clusterId, picked);
      await this.loadTree(picked);
    } catch (e) {
      this.errorMsg.set(String(e));
    }
  }

  private async loadTree(folderPath: string): Promise<void> {
    this.loading.set(true);
    try {
      const entries = await this.tauri.readDir(folderPath);
      this.tree.set(this.sortEntries(entries));
      this.rootPath.set(folderPath);
    } catch (e) {
      this.errorMsg.set(String(e));
    } finally {
      this.loading.set(false);
    }
  }

  async clickNode(entry: DirEntry): Promise<void> {
    if (entry.isDir) {
      await this.toggleDir(entry);
    } else {
      await this.openFile(entry);
    }
  }

  async toggleDir(entry: DirEntry): Promise<void> {
    const expanded = new Set(this.expandedPaths());
    if (expanded.has(entry.path)) {
      expanded.delete(entry.path);
      this.expandedPaths.set(expanded);
      return;
    }
    if (!entry.childrenLoaded) {
      try {
        const children = await this.tauri.readDir(entry.path);
        entry.children = this.sortEntries(children);
        entry.childrenLoaded = true;
      } catch (e) {
        this.errorMsg.set(String(e));
        return;
      }
    }
    expanded.add(entry.path);
    this.expandedPaths.set(expanded);
    // Trigger flatNodes recomputation
    this.tree.update(t => [...t]);
  }

  private async openFile(entry: DirEntry): Promise<void> {
    const existing = this.openTabs().find(t => t.path === entry.path);
    if (existing) {
      this.activeTabPath.set(entry.path);
      return;
    }
    try {
      const content = await this.tauri.readTextFile(entry.path);
      this.openTabs.update(tabs => [...tabs, {
        path: entry.path,
        name: entry.name,
        content,
        originalContent: content,
        dirty: false,
      }]);
      this.activeTabPath.set(entry.path);
    } catch (e) {
      this.errorMsg.set(String(e));
    }
  }

  activateTab(path: string): void {
    this.activeTabPath.set(path);
  }

  async closeTab(path: string, event: Event): Promise<void> {
    event.stopPropagation();
    const tabs = this.openTabs();
    const idx = tabs.findIndex(t => t.path === path);
    if (idx === -1) return;
    const target = tabs[idx];
    if (target.dirty) {
      const ok = await this.tauri.confirm(
        `"${target.name}" has unsaved changes.\nClose without saving?`
      );
      if (!ok) return;
    }
    const newTabs = tabs.filter(t => t.path !== path);
    this.openTabs.set(newTabs);
    if (this.activeTabPath() === path) {
      const next = newTabs[idx] ?? newTabs[Math.max(0, idx - 1)] ?? null;
      this.activeTabPath.set(next?.path ?? null);
    }
  }

  async saveActiveFile(): Promise<void> {
    const tab = this.activeTab();
    if (!tab) return;
    try {
      await this.tauri.writeTextFile(tab.path, tab.content);
      this.openTabs.update(tabs =>
        tabs.map(t => t.path === tab.path
          ? { ...t, originalContent: tab.content, dirty: false }
          : t)
      );
    } catch (e) {
      this.errorMsg.set(String(e));
    }
  }

  applyActive(): void {
    const tab = this.activeTab();
    if (!tab) return;
    this.applyFile.emit({ path: tab.path, content: tab.content });
  }

  isYaml(path: string): boolean {
    return path.endsWith('.yaml') || path.endsWith('.yml');
  }

  isExpanded(path: string): boolean {
    return this.expandedPaths().has(path);
  }

  private sortEntries(entries: DirEntry[]): DirEntry[] {
    return [...entries].sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
  }

  @HostListener('window:keydown', ['$event'])
  onKeydown(event: KeyboardEvent): void {
    if ((event.metaKey || event.ctrlKey) && event.key === 's') {
      if (this.activeTab()) {
        event.preventDefault();
        this.saveActiveFile();
      }
    }
  }
}

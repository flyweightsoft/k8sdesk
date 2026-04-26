import { Injectable, signal } from '@angular/core';

export interface Theme {
  id: string;
  name: string;
  mode: 'dark' | 'light';
  description: string;
  /** 4 preview swatches: [bg, panel, accent, ok-color] */
  swatches: [string, string, string, string];
}

export const THEMES: Theme[] = [
  {
    id: 'github-dark',
    name: 'GitHub Dark',
    mode: 'dark',
    description: 'Classic GitHub dark interface',
    swatches: ['#0d1117', '#161b22', '#58a6ff', '#7ee787'],
  },
  {
    id: 'midnight',
    name: 'Midnight',
    mode: 'dark',
    description: 'Deep navy with cool blue accents',
    swatches: ['#080c14', '#0e1420', '#7c9ef5', '#5fd77b'],
  },
  {
    id: 'solarized-dark',
    name: 'Solarized Dark',
    mode: 'dark',
    description: 'Warm teal tones, easy on the eyes',
    swatches: ['#002b36', '#073642', '#268bd2', '#859900'],
  },
  {
    id: 'github-light',
    name: 'GitHub Light',
    mode: 'light',
    description: 'Clean and minimal light interface',
    swatches: ['#ffffff', '#f6f8fa', '#0969da', '#1a7f37'],
  },
  {
    id: 'one-light',
    name: 'One Light',
    mode: 'light',
    description: 'Editor-inspired warm light theme',
    swatches: ['#fafafa', '#f0f0f0', '#4078f2', '#50a14f'],
  },
];

const STORAGE_KEY = 'k8sdesk-theme';
const PREF_KEY = (mode: string) => `k8sdesk-pref-${mode}`;
const DEFAULT_THEME = 'github-dark';

@Injectable({ providedIn: 'root' })
export class ThemeService {
  readonly themes = THEMES;
  readonly darkThemes  = THEMES.filter(t => t.mode === 'dark');
  readonly lightThemes = THEMES.filter(t => t.mode === 'light');
  readonly current = signal<Theme>(this._load());

  private _load(): Theme {
    const saved = localStorage.getItem(STORAGE_KEY);
    return THEMES.find(t => t.id === saved) ?? THEMES.find(t => t.id === DEFAULT_THEME)!;
  }

  apply(themeId: string): void {
    const theme = THEMES.find(t => t.id === themeId);
    if (!theme) return;
    document.documentElement.setAttribute('data-theme', theme.id);
    localStorage.setItem(STORAGE_KEY, theme.id);
    localStorage.setItem(PREF_KEY(theme.mode), theme.id);
    this.current.set(theme);
  }

  toggleMode(): void {
    const cur = this.current();
    const targetMode = cur.mode === 'dark' ? 'light' : 'dark';
    const prefId = localStorage.getItem(PREF_KEY(targetMode));
    const target = THEMES.find(t => t.id === prefId && t.mode === targetMode)
      ?? THEMES.find(t => t.mode === targetMode)!;
    this.apply(target.id);
  }

  init(): void {
    this.apply(this.current().id);
  }
}


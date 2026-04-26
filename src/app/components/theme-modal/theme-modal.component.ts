import { Component, EventEmitter, Output, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { ThemeService, Theme } from '../../services/theme.service';

@Component({
  selector: 'app-theme-modal',
  standalone: true,
  imports: [CommonModule],
  templateUrl: './theme-modal.component.html',
  styleUrl: './theme-modal.component.scss',
})
export class ThemeModalComponent {
  @Output() closed = new EventEmitter<void>();

  readonly theme = inject(ThemeService);

  select(t: Theme): void {
    this.theme.apply(t.id);
  }
}

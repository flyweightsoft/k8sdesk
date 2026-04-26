import { CommonModule } from '@angular/common';
import {
  ChangeDetectionStrategy,
  Component,
  EventEmitter,
  Input,
  Output,
  signal,
  computed,
} from '@angular/core';
import { FormsModule } from '@angular/forms';
import { ConfirmationRequest } from '../../models/cluster';

@Component({
  selector: 'app-confirm-modal',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [CommonModule, FormsModule],
  templateUrl: './confirm-modal.component.html',
  styleUrl: './confirm-modal.component.scss',
})
export class ConfirmModalComponent {
  @Input({ required: true }) request!: ConfirmationRequest;
  @Output() confirmed = new EventEmitter<string>();
  @Output() cancelled = new EventEmitter<void>();

  typed = signal('');

  canProceed = computed(() => {
    if (!this.request.require_typed_name) return true;
    return this.typed().trim() === this.request.cluster_name;
  });

  proceed(): void {
    if (!this.canProceed()) return;
    this.confirmed.emit(this.request.challenge_id);
  }

  cancel(): void {
    this.cancelled.emit();
  }
}

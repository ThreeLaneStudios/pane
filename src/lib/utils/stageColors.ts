/**
 * Shared stage/status colors and labels.
 * Single source of truth — imported by Card, KanbanBoard, AllTasksView, Editor.
 */

export const STAGE_COLORS: Record<string, string> = {
  cabinet: '#888780',
  todo: '#378ADD',
  in_progress: '#EF9F27',
  review: '#7F77DD',
  done: '#639922',
};

export function getStageColor(status: string | null | undefined): string {
  return STAGE_COLORS[status || ''] || STAGE_COLORS.cabinet;
}

export function getStageLabel(status: string | null | undefined): string {
  switch (status) {
    case 'cabinet': return 'Cabinet';
    case 'todo': return 'To Do';
    case 'in_progress': return 'In Progress';
    case 'review': return 'Review';
    case 'done': return 'Done';
    default: return 'Cabinet';
  }
}

export interface UserDashboardConfig {
  id: string;
  onboarding_completed: boolean;
  onboarding_template?: string;
  theme_variant: string;
  created_at: string;
  updated_at: string;
}

export interface DashboardModule {
  id: string;
  module_type: string;
  sort_order: number;
  is_enabled: boolean;
  config_json: string;
  static_content?: string;
  items: ModuleItem[];
  created_at: string;
  updated_at: string;
}

export interface ModuleItem {
  id: string;
  module_id: string;
  item_type: string;
  label: string;
  description?: string;
  emoji?: string;
  config_json: string;
  sort_order: number;
  is_active: boolean;
}

export interface DailyEntry {
  id: string;
  module_id: string;
  entry_date: string;
  data_json: string;
  completed_at?: string;
  created_at: string;
  updated_at: string;
}

export interface HabitStreak {
  current_streak: number;
  longest_streak: number;
  completion_rate_30d: number;
}

export interface HabitStatus {
  item_id: string;
  label: string;
  emoji?: string;
  completed: boolean;
  streak: number;
}

import { CalendarDays, FileText, House, Library, ListTodo, Network, Users } from 'lucide-react';

// Shared by the dock, expanded navigation, and navigation preview.
export const PRIMARY_DESTINATIONS = [
  { id: 'ask', label: 'Home', icon: House },
  { id: 'today', label: 'Schedule', icon: ListTodo },
  { id: 'calendar', label: 'Calendar', icon: CalendarDays },
  { id: 'documents', label: 'Documents', icon: FileText },
  { id: 'library', label: 'Library', icon: Library },
  { id: 'team', label: 'Team', icon: Users },
  { id: 'knowledge', label: 'Knowledge', icon: Network },
] as const;

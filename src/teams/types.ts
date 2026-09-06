export type TeamRole = "owner" | "admin" | "member";
export type SpaceRole = "viewer" | "editor";
export type TeamUser = {
  id: string;
  name: string;
  title?: string;
  about?: string;
  avatar_version?: string;
};
export type TeamProfile = TeamUser & { avatar_data: string; revision: number };
export type TeamReaction = {
  emoji: string;
  count: number;
  reacted: boolean;
  names: string[];
};
export type TeamOrg = { id: string; name: string; role: TeamRole };
export type TeamSpace = {
  id: string;
  org_id: string;
  name: string;
  description: string;
  visibility: "team" | "restricted";
  role: SpaceRole;
  api_enabled: boolean;
};
export type TeamFolder = {
  id: string;
  space_id: string;
  parent_id: string | null;
  name: string;
  description: string;
};
export type TeamMember = TeamUser & { role: TeamRole };
export type TeamGroup = { id: string; name: string; member_ids: string[] };
export type TeamGrant = {
  id: string;
  name: string;
  kind: "member" | "group";
  role: SpaceRole;
};
export type TeamRecipe = {
  id: string;
  name: string;
  prompt: string;
  kind: "recipe" | "template";
  owner_id: string;
  revision: number;
};
export type TeamNote = {
  id: string;
  space_id: string;
  owner_id: string;
  owner_name: string;
  title: string;
  summary: string;
  transcript: string;
  occurred_at: string;
  published_at: string;
  updated_at: string;
  revision: number;
  trashed_at: string | null;
  folder_ids: string[];
  can_edit: boolean;
  can_manage: boolean;
};
export type TeamNoteRow = Omit<TeamNote, "summary" | "transcript"> & {
  excerpt: string;
  has_transcript: boolean;
};
export type TeamSnapshot = {
  access_version: number;
  org: TeamOrg;
  user: TeamUser;
  spaces: TeamSpace[];
  folders: TeamFolder[];
  members: TeamMember[];
  recipes: TeamRecipe[];
};
export type TeamSession = { server: string; connected: boolean };
export type TeamSource = {
  id: string;
  title: string;
  revision: number;
  citation: string;
  excerpt: string;
};
export type TeamAnswer = {
  answer: string;
  sources: TeamSource[];
  limited: boolean;
  conversation?: TeamConversation;
};
export type TeamTurn = Omit<TeamAnswer, "conversation"> & {
  id: string;
  question: string;
  created_at: string;
};
export type TeamConversation = {
  id: string;
  revision: number;
  scope: { space_id: string; folder_id: string; note_ids: string[] };
  turns: TeamTurn[];
  updated_at: string;
};
export type TeamConversationRow = {
  id: string;
  question: string;
  updated_at: string;
  available: boolean;
};

export type TeamChatRoom = {
  message_extras?: boolean;
  id: string;
  org_id: string;
  kind: "channel" | "direct";
  name: string;
  description: string;
  created_by: string;
  created_at: string;
  archived_at: string | null;
  revision: number;
  is_default: boolean;
  participants: (TeamUser & { active: boolean })[];
  unread: number;
  unread_mentions?: number;
  latest_unread_mention_seq?: number;
  notification_cursor?: number;
  notification_user_id?: string;
  last_activity: string;
  last_message?: {
    author_id: string;
    author_name: string;
    body: string;
    created_at: string;
  } | null;
  can_manage: boolean;
  can_send: boolean;
};
export type TeamChatMessage = {
  id: string;
  room_id: string;
  author_id: string;
  author_name: string;
  body: string;
  thread_id?: string | null;
  reply_count?: number;
  last_reply_at?: string | null;
  reactions?: TeamReaction[];
  created_at: string;
  edited_at: string | null;
  deleted_at: string | null;
  revision: number;
  created_seq: number;
  can_edit: boolean;
  can_delete: boolean;
};
export type TeamChatPage = {
  live?: boolean;
  parent?: TeamChatMessage;
  room: TeamChatRoom;
  messages: TeamChatMessage[];
  cursor: number;
  has_more: boolean;
  older_before: number | null;
};

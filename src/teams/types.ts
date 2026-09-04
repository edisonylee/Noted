export type TeamRole = "owner" | "admin" | "member";
export type SpaceRole = "viewer" | "editor";
export type TeamUser = { id: string; name: string };
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

import {
  TASK_DOCUMENT_VERSION,
  documentPlainText,
  extractDocumentTasks,
  type StructuredDocument,
} from "./editor/document";

export type DailyTaskBlock = {
  task: string;
  start?: string;
  end?: string;
  duration_min?: number;
};

type DailyTaskData = {
  task_doc_version: number;
  task_doc: StructuredDocument;
  todos: ReturnType<typeof extractDocumentTasks>;
};

type DailyTaskSaveArgs = {
  raw_text: string;
  source: "text";
  event_date: string;
  entries: Array<{
    category: "schedule";
    description: string;
    data: DailyTaskData & { blocks: DailyTaskBlock[] };
  }>;
};

export type DailyTaskPersistence = {
  updateEntry: (entryId: number, data: DailyTaskData) => Promise<unknown>;
  createEntry: (args: DailyTaskSaveArgs) => Promise<unknown>;
  refreshEntries: () => void | Promise<void>;
};

/**
 * Persist one day's task document, then refresh the schedule cache regardless
 * of whether this updated an existing entry or created a new one.
 */
export async function persistDailyTaskDocument({
  entryId,
  targetDate,
  document,
  blocks,
  persistence,
}: {
  entryId: number | null;
  targetDate: string;
  document: StructuredDocument;
  blocks: DailyTaskBlock[];
  persistence: DailyTaskPersistence;
}): Promise<void> {
  const data: DailyTaskData = {
    task_doc_version: TASK_DOCUMENT_VERSION,
    task_doc: document,
    todos: extractDocumentTasks(document),
  };

  if (entryId != null) {
    await persistence.updateEntry(entryId, data);
  } else {
    await persistence.createEntry({
      raw_text: documentPlainText(document),
      source: "text",
      event_date: targetDate,
      entries: [
        {
          category: "schedule",
          description: "daily schedule and tasks",
          data: { blocks, ...data },
        },
      ],
    });
  }

  await persistence.refreshEntries();
}

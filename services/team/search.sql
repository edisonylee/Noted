CREATE VIRTUAL TABLE IF NOT EXISTS chat_messages_fts USING fts5(
 body, content='chat_messages', content_rowid='rowid',
 tokenize="unicode61 remove_diacritics 2"
);
CREATE TRIGGER IF NOT EXISTS chat_messages_fts_ai AFTER INSERT ON chat_messages BEGIN
 INSERT INTO chat_messages_fts(rowid,body) VALUES(new.rowid,new.body);
END;
CREATE TRIGGER IF NOT EXISTS chat_messages_fts_ad AFTER DELETE ON chat_messages BEGIN
 INSERT INTO chat_messages_fts(chat_messages_fts,rowid,body) VALUES('delete',old.rowid,old.body);
END;
CREATE TRIGGER IF NOT EXISTS chat_messages_fts_au AFTER UPDATE ON chat_messages BEGIN
 INSERT INTO chat_messages_fts(chat_messages_fts,rowid,body) VALUES('delete',old.rowid,old.body);
 INSERT INTO chat_messages_fts(rowid,body) VALUES(new.rowid,new.body);
END;
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
 title,summary,transcript, content='notes', content_rowid='rowid',
 tokenize="unicode61 remove_diacritics 2"
);
CREATE TRIGGER IF NOT EXISTS notes_fts_ai AFTER INSERT ON notes BEGIN
 INSERT INTO notes_fts(rowid,title,summary,transcript) VALUES(new.rowid,new.title,new.summary,new.transcript);
END;
CREATE TRIGGER IF NOT EXISTS notes_fts_ad AFTER DELETE ON notes BEGIN
 INSERT INTO notes_fts(notes_fts,rowid,title,summary,transcript) VALUES('delete',old.rowid,old.title,old.summary,old.transcript);
END;
CREATE TRIGGER IF NOT EXISTS notes_fts_au AFTER UPDATE ON notes BEGIN
 INSERT INTO notes_fts(notes_fts,rowid,title,summary,transcript) VALUES('delete',old.rowid,old.title,old.summary,old.transcript);
 INSERT INTO notes_fts(rowid,title,summary,transcript) VALUES(new.rowid,new.title,new.summary,new.transcript);
END;

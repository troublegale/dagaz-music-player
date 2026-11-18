CREATE TABLE server_meta (key TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL);
CREATE TABLE artists (
    id TEXT PRIMARY KEY NOT NULL, normalized_name TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL, data_json TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 1,
    deleted INTEGER NOT NULL DEFAULT 0,last_sequence INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX artist_listing ON artists(deleted,normalized_name,id);
CREATE TABLE albums (
    id TEXT PRIMARY KEY NOT NULL,identity_key TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,normalized_name TEXT NOT NULL,album_artist_id TEXT REFERENCES artists(id),
    data_json TEXT NOT NULL,revision INTEGER NOT NULL DEFAULT 1,deleted INTEGER NOT NULL DEFAULT 0,last_sequence INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX album_listing ON albums(deleted,normalized_name,id);
ALTER TABLE tracks ADD COLUMN artist_id TEXT REFERENCES artists(id);
ALTER TABLE tracks ADD COLUMN album_id TEXT REFERENCES albums(id);
ALTER TABLE tracks ADD COLUMN sort_title TEXT NOT NULL DEFAULT '';
ALTER TABLE tracks ADD COLUMN sort_artist TEXT NOT NULL DEFAULT '';
ALTER TABLE tracks ADD COLUMN sort_album TEXT NOT NULL DEFAULT '';
ALTER TABLE tracks ADD COLUMN sort_album_order TEXT NOT NULL DEFAULT '';
ALTER TABLE tracks ADD COLUMN catalog_ready INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tracks ADD COLUMN last_sequence INTEGER NOT NULL DEFAULT 0;
CREATE INDEX track_upgrade ON tracks(catalog_ready,deleted,id);
CREATE INDEX track_title_listing ON tracks(deleted,sort_title,id);
CREATE INDEX track_artist_listing ON tracks(deleted,sort_artist,id);
CREATE INDEX track_album_listing ON tracks(deleted,sort_album,id);
CREATE INDEX album_track_listing ON tracks(deleted,album_id,sort_album_order,id);
CREATE INDEX track_artist_id ON tracks(artist_id,deleted);
CREATE VIRTUAL TABLE track_search USING fts5(title,artist,album,album_artist,tokenize='unicode61');
CREATE TRIGGER track_search_insert AFTER INSERT ON tracks WHEN new.deleted=0 BEGIN
    INSERT INTO track_search(rowid,title,artist,album,album_artist) VALUES(new.rowid,COALESCE(json_extract(new.data_json,'$.title'),new.filename),COALESCE(json_extract(new.data_json,'$.artist'),''),COALESCE(json_extract(new.data_json,'$.album'),''),COALESCE(json_extract(new.data_json,'$.album_artist'),''));
END;
CREATE TRIGGER track_search_update AFTER UPDATE OF data_json,deleted ON tracks BEGIN
    DELETE FROM track_search WHERE rowid=old.rowid;
    INSERT INTO track_search(rowid,title,artist,album,album_artist) SELECT new.rowid,COALESCE(json_extract(new.data_json,'$.title'),new.filename),COALESCE(json_extract(new.data_json,'$.artist'),''),COALESCE(json_extract(new.data_json,'$.album'),''),COALESCE(json_extract(new.data_json,'$.album_artist'),'') WHERE new.deleted=0;
END;
INSERT INTO track_search(rowid,title,artist,album,album_artist) SELECT rowid,COALESCE(json_extract(data_json,'$.title'),filename),COALESCE(json_extract(data_json,'$.artist'),''),COALESCE(json_extract(data_json,'$.album'),''),COALESCE(json_extract(data_json,'$.album_artist'),'') FROM tracks WHERE deleted=0;
CREATE TABLE playlists (
    id TEXT PRIMARY KEY NOT NULL,name TEXT NOT NULL,normalized_name TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,track_count INTEGER NOT NULL DEFAULT 0,
    data_json TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0,last_sequence INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX playlist_listing ON playlists(deleted,normalized_name,id);
CREATE TABLE playlist_entries (
    playlist_id TEXT NOT NULL REFERENCES playlists(id),position INTEGER NOT NULL,
    track_id TEXT NOT NULL REFERENCES tracks(id),PRIMARY KEY(playlist_id,position)
);
CREATE INDEX playlist_track ON playlist_entries(track_id,playlist_id);

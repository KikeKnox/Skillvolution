CREATE TABLE skills (
    id TEXT PRIMARY KEY,
    scope TEXT,
    deprecated INTEGER NOT NULL DEFAULT 0 CHECK(deprecated IN (0, 1))
);

CREATE TABLE revisions (
    id TEXT NOT NULL REFERENCES skills(id),
    version INTEGER NOT NULL CHECK(version > 0),
    description TEXT NOT NULL,
    tags TEXT NOT NULL,
    content TEXT NOT NULL,
    evidence TEXT NOT NULL,
    expected_version INTEGER NOT NULL CHECK(expected_version >= 0),
    status TEXT NOT NULL DEFAULT 'draft' CHECK(status IN ('draft', 'published', 'rejected', 'superseded')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    reviewed_at TEXT,
    review_note TEXT,
    PRIMARY KEY (id, version)
);

CREATE TABLE outcomes (
    id TEXT NOT NULL,
    version INTEGER NOT NULL,
    result TEXT NOT NULL CHECK(result IN ('helped', 'failed', 'not_applicable')),
    note TEXT NOT NULL,
    project TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    FOREIGN KEY (id, version) REFERENCES revisions(id, version)
);

CREATE INDEX outcomes_by_revision ON outcomes(id, version, result);

CREATE TABLE hook_state (
    session_id TEXT PRIMARY KEY,
    transcript_offset INTEGER NOT NULL
);

CREATE VIRTUAL TABLE skills_fts USING fts5(
    id, description, tags, content,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE VIEW current_skills AS
SELECT r.id, r.version, r.description, r.tags, r.content, s.scope, s.deprecated,
    (SELECT COUNT(*) FROM outcomes o WHERE o.id = r.id AND o.version = r.version AND o.result = 'helped') AS helped,
    (SELECT COUNT(*) FROM outcomes o WHERE o.id = r.id AND o.version = r.version AND o.result = 'failed') AS failed,
    (SELECT COUNT(*) FROM outcomes o WHERE o.id = r.id AND o.version = r.version AND o.result = 'not_applicable') AS not_applicable
FROM skills s
JOIN revisions r ON r.id = s.id
WHERE r.status = 'published'
    AND r.version = (SELECT MAX(p.version) FROM revisions p WHERE p.id = r.id AND p.status = 'published');

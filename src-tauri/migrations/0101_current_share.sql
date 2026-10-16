-- One recoverable share only. Message attachment refs and this terminal state commit together.
CREATE TABLE IF NOT EXISTS current_share (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    record TEXT NOT NULL
);

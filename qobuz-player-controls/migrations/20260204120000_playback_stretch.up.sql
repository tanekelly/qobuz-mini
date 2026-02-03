ALTER TABLE configuration ADD COLUMN time_stretch_ratio REAL NOT NULL DEFAULT 1.0;
ALTER TABLE configuration ADD COLUMN pitch_semitones INTEGER NOT NULL DEFAULT 0;

-- Owner data the feeders do not hold: pets, the foods they eat, which
-- feeder serves which pet and food, and refills.

-- `species` is its stored spelling; `birth_date` an ISO date.
CREATE TABLE pets (
    id BLOB PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    species TEXT NOT NULL,
    birth_date TEXT,
    daily_kcal INTEGER,
    created_ms INTEGER NOT NULL
) STRICT;

-- The weight log. An entry may be backdated (a vet's reading), so the log
-- is ordered by `at_ms`, with the id breaking ties.
CREATE TABLE pet_weights (
    id BLOB PRIMARY KEY NOT NULL,
    pet_id BLOB NOT NULL REFERENCES pets (id) ON DELETE CASCADE,
    at_ms INTEGER NOT NULL,
    grams INTEGER NOT NULL
) STRICT;

CREATE INDEX pet_weights_by_time ON pet_weights (pet_id, at_ms, id);

-- Pets and feeders are many-to-many, and the links keep their history: a
-- feeder's intake at a time is split among the pets it served then. A link
-- runs from `from_ms` until `to_ms`, open while that is NULL. A pair has at
-- most one open link, and a new one never starts before the pair's last
-- one ended, so a pair's links never overlap.
CREATE TABLE pet_feeders (
    seq INTEGER PRIMARY KEY,
    pet_id BLOB NOT NULL REFERENCES pets (id) ON DELETE CASCADE,
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    from_ms INTEGER NOT NULL,
    to_ms INTEGER,
    CHECK (to_ms IS NULL OR to_ms >= from_ms)
) STRICT;

CREATE UNIQUE INDEX pet_feeders_open ON pet_feeders (pet_id, feeder_id) WHERE to_ms IS NULL;
CREATE INDEX pet_feeders_by_feeder ON pet_feeders (feeder_id, from_ms);

-- Energy is kcal per kg, as labels print it; a portion is a fraction of
-- `grams_per_cup`.
CREATE TABLE foods (
    id BLOB PRIMARY KEY NOT NULL,
    brand TEXT NOT NULL,
    name TEXT NOT NULL,
    kcal_per_kg INTEGER NOT NULL,
    bag_grams INTEGER NOT NULL,
    grams_per_cup INTEGER NOT NULL,
    created_ms INTEGER NOT NULL
) STRICT;

-- Each feeder's food from `from_ms` on, until its next row. `from_ms`
-- never decreases along `seq` (a change is never dated before the one it
-- follows), so the food at a time is the last row that started by then.
-- A food named here cannot be deleted: past feeds' calories use it.
CREATE TABLE feeder_foods (
    seq INTEGER PRIMARY KEY,
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    food_id BLOB NOT NULL REFERENCES foods (id),
    from_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX feeder_foods_by_feeder ON feeder_foods (feeder_id, seq);

-- `grams` is what went in: a full bag's is the food's bag size when it was
-- recorded. `food_id` is the feeder's food then, if it had one. `user_id`
-- names no foreign key: a refill outlives the account that recorded it.
CREATE TABLE refills (
    seq INTEGER PRIMARY KEY,
    id BLOB NOT NULL UNIQUE,
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    at_ms INTEGER NOT NULL,
    grams INTEGER NOT NULL,
    full_bag INTEGER NOT NULL,
    food_id BLOB REFERENCES foods (id),
    user_id BLOB NOT NULL
) STRICT;

CREATE INDEX refills_by_feeder ON refills (feeder_id, seq);

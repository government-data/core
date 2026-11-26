-- Module events (quality-sdk decision 0017): what a module publishes from
-- its outbox, stored in order (`seq`), once per id (a publish told twice is
-- stored once), with the topic and version it declared and when it
-- happened. `data` is the module's, validated against the declared schema
-- and never interpreted or logged by the core; it carries ids, not personal
-- data. Subscribers read a topic's events after a cursor; `pg_notify` on
-- `module_events` (the topic) after a commit wakes those that wait. An
-- expansion: two new tables.
CREATE TABLE public.module_events (
    seq bigserial PRIMARY KEY,
    id uuid NOT NULL UNIQUE,
    topic text NOT NULL,
    version integer NOT NULL CHECK (version > 0),
    publisher text NOT NULL,
    occurred_at timestamp with time zone NOT NULL,
    stored_at timestamp with time zone NOT NULL DEFAULT now(),
    data jsonb NOT NULL
);

CREATE INDEX module_events_by_topic ON public.module_events (topic, version, seq);

-- How far each consuming module confirmed it handled a topic at a version:
-- one cursor per module, topic and version. A removed module's cursors are
-- kept (its subscription paused); only its purge deletes them. Retention
-- deletes an event once every consumer of its topic and version confirmed
-- past it and 30 days went by.
CREATE TABLE public.module_event_cursors (
    module text NOT NULL,
    topic text NOT NULL,
    version integer NOT NULL CHECK (version > 0),
    cursor bigint NOT NULL DEFAULT 0,
    confirmed_at timestamp with time zone NOT NULL DEFAULT now(),
    PRIMARY KEY (module, topic, version)
);

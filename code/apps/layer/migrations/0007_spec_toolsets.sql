-- A repository's spec toolset and how a task's spec reaches it. `spec_commit`
-- replaces `specs_in_app`: a repository that kept specs in the app takes
-- `none`, every other one the default, `whole`.
ALTER TABLE repositories
    ADD COLUMN spec_toolset text,
    ADD COLUMN spec_commit text NOT NULL DEFAULT 'whole'
        CHECK (spec_commit IN ('whole', 'own', 'none'));
UPDATE repositories SET spec_commit = 'none' WHERE specs_in_app;
ALTER TABLE repositories DROP COLUMN specs_in_app;

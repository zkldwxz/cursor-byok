-- Cached failure placeholders are pinned results of a broken reader, not image
-- descriptions. Deleting them lets a newly configured vision model re-read the
-- images on the next turn.
DELETE FROM image_descriptions WHERE description LIKE '[Image not analyzed:%';

-- Where an automatically filled price came from; NULL means the user entered it,
-- and a user's price is never replaced automatically.
ALTER TABLE model_prices ADD COLUMN source TEXT CHECK (source <> '');

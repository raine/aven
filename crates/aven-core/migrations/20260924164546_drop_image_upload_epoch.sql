-- Image upload tickets carry random per-device reservations that expire, and
-- prune skips objects with live tickets, so an object epoch fences nothing.
ALTER TABLE server_e2ee_image_tickets DROP COLUMN epoch;
ALTER TABLE server_e2ee_images DROP COLUMN epoch;

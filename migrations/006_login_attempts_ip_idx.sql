-- Support IP-based login rate limiting: count failed attempts from one client
-- IP within a time window, regardless of username (password-spray throttling).
CREATE INDEX IF NOT EXISTS local_auth_attempts_ip_idx
ON local_auth_attempts(ip, occurred_at);

CREATE TABLE IF NOT EXISTS audit_logs (
    id TEXT PRIMARY KEY,
    timestamp TEXT NOT NULL,
    admin_subject TEXT NOT NULL,
    admin_username TEXT NOT NULL,
    target_keycloak_user_id TEXT,
    target_matrix_user_id TEXT,
    action TEXT NOT NULL,
    result TEXT NOT NULL,
    metadata_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_logs_timestamp ON audit_logs(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_logs_target_keycloak_user_id ON audit_logs(target_keycloak_user_id);
CREATE INDEX IF NOT EXISTS idx_audit_logs_target_matrix_user_id ON audit_logs(target_matrix_user_id);

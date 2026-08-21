"""Version-aware Mindustry compatibility tooling (schema v2)."""

SCHEMA_VERSION = 2
GENERATOR_NAME = "tools/mindustry_manifest.py"
GENERATOR_VERSION = "2.0.0"

TERMINAL_STATUSES = frozenset(
    {
        "VERIFIED_IMPLEMENTED",
        "VERIFIED_UNCHANGED",
        "CLIENT_ONLY",
        "EDITOR_ONLY",
        "OUT_OF_SCOPE_EXPLICIT",
    }
)

NON_TERMINAL_STATUSES = frozenset(
    {
        "DISCOVERED",
        "REOPENED",
        "IMPLEMENTATION_REQUIRED",
        "EVIDENCE_REQUIRED",
        "UNKNOWN_REQUIRES_PROBE",
        "BLOCKED",
    }
)

SERVER_AUTHORITATIVE_CATEGORIES = frozenset(
    {
        "WIRE_PACKET",
        "STREAMING",
        "RPC",
        "TYPEIO",
        "SAVE",
        "MAP",
        "CONTENT",
        "ENTITY_SYNC",
        "RULES",
        "PLACEMENT",
        "LOGIC",
        "STATUS",
        "AI",
        "UNITS",
        "PHYSICS_COLLISION",
        "COMBAT",
        "ECONOMY",
        "STATEFUL_BUILDING",
        "INPUT_AUTHORITY",
        "ADMIN",
        "UNKNOWN_REQUIRES_PROBE",
        "JOIN",
        "SNAPSHOT",
    }
)

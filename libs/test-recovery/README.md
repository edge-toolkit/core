# Recovery test helpers

`et-test-recovery` provides the example configuration, lifecycle event builders and policy fixtures used by
recovery integration tests. It is a development dependency, not part of the deployed supervisor. Recovery-specific
fixtures stay here so the general WebSocket test server does not depend on recovery policy.

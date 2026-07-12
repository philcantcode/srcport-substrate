from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Iterable as _Iterable, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Lifecycle(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    LIFECYCLE_UNSPECIFIED: _ClassVar[Lifecycle]
    LIFECYCLE_REGISTERED: _ClassVar[Lifecycle]
    LIFECYCLE_LOADED: _ClassVar[Lifecycle]
    LIFECYCLE_ACTIVE: _ClassVar[Lifecycle]
    LIFECYCLE_DEACTIVATED: _ClassVar[Lifecycle]

class RunState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    RUN_STATE_UNSPECIFIED: _ClassVar[RunState]
    RUN_STATE_RUNNING: _ClassVar[RunState]
    RUN_STATE_COMPLETED: _ClassVar[RunState]
    RUN_STATE_STALLED: _ClassVar[RunState]
    RUN_STATE_FAILED: _ClassVar[RunState]
    RUN_STATE_CANCELLED: _ClassVar[RunState]

class ErrorCode(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    ERROR_CODE_UNSPECIFIED: _ClassVar[ErrorCode]
    ERROR_CODE_NOT_FOUND: _ClassVar[ErrorCode]
    ERROR_CODE_INVALID: _ClassVar[ErrorCode]
    ERROR_CODE_CONFLICT: _ClassVar[ErrorCode]
    ERROR_CODE_FAILED_PRECONDITION: _ClassVar[ErrorCode]
    ERROR_CODE_RESOURCE_EXHAUSTED: _ClassVar[ErrorCode]
    ERROR_CODE_BLOB_INTEGRITY: _ClassVar[ErrorCode]
LIFECYCLE_UNSPECIFIED: Lifecycle
LIFECYCLE_REGISTERED: Lifecycle
LIFECYCLE_LOADED: Lifecycle
LIFECYCLE_ACTIVE: Lifecycle
LIFECYCLE_DEACTIVATED: Lifecycle
RUN_STATE_UNSPECIFIED: RunState
RUN_STATE_RUNNING: RunState
RUN_STATE_COMPLETED: RunState
RUN_STATE_STALLED: RunState
RUN_STATE_FAILED: RunState
RUN_STATE_CANCELLED: RunState
ERROR_CODE_UNSPECIFIED: ErrorCode
ERROR_CODE_NOT_FOUND: ErrorCode
ERROR_CODE_INVALID: ErrorCode
ERROR_CODE_CONFLICT: ErrorCode
ERROR_CODE_FAILED_PRECONDITION: ErrorCode
ERROR_CODE_RESOURCE_EXHAUSTED: ErrorCode
ERROR_CODE_BLOB_INTEGRITY: ErrorCode

class Capability(_message.Message):
    __slots__ = ("name", "contract", "inputs", "outputs")
    NAME_FIELD_NUMBER: _ClassVar[int]
    CONTRACT_FIELD_NUMBER: _ClassVar[int]
    INPUTS_FIELD_NUMBER: _ClassVar[int]
    OUTPUTS_FIELD_NUMBER: _ClassVar[int]
    name: str
    contract: str
    inputs: _containers.RepeatedCompositeFieldContainer[Port]
    outputs: _containers.RepeatedCompositeFieldContainer[Port]
    def __init__(self, name: _Optional[str] = ..., contract: _Optional[str] = ..., inputs: _Optional[_Iterable[_Union[Port, _Mapping]]] = ..., outputs: _Optional[_Iterable[_Union[Port, _Mapping]]] = ...) -> None: ...

class Port(_message.Message):
    __slots__ = ("name", "contract", "multiple", "optional")
    NAME_FIELD_NUMBER: _ClassVar[int]
    CONTRACT_FIELD_NUMBER: _ClassVar[int]
    MULTIPLE_FIELD_NUMBER: _ClassVar[int]
    OPTIONAL_FIELD_NUMBER: _ClassVar[int]
    name: str
    contract: str
    multiple: bool
    optional: bool
    def __init__(self, name: _Optional[str] = ..., contract: _Optional[str] = ..., multiple: bool = ..., optional: bool = ...) -> None: ...

class ModuleManifest(_message.Message):
    __slots__ = ("name", "version", "provides", "requires")
    NAME_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    PROVIDES_FIELD_NUMBER: _ClassVar[int]
    REQUIRES_FIELD_NUMBER: _ClassVar[int]
    name: str
    version: str
    provides: _containers.RepeatedCompositeFieldContainer[Capability]
    requires: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, name: _Optional[str] = ..., version: _Optional[str] = ..., provides: _Optional[_Iterable[_Union[Capability, _Mapping]]] = ..., requires: _Optional[_Iterable[str]] = ...) -> None: ...

class BlobRef(_message.Message):
    __slots__ = ("digest", "byte_count", "namespace")
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    BYTE_COUNT_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    digest: str
    byte_count: int
    namespace: str
    def __init__(self, digest: _Optional[str] = ..., byte_count: _Optional[int] = ..., namespace: _Optional[str] = ...) -> None: ...

class ObjectRef(_message.Message):
    __slots__ = ("digest", "byte_count", "namespace")
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    BYTE_COUNT_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    digest: str
    byte_count: int
    namespace: str
    def __init__(self, digest: _Optional[str] = ..., byte_count: _Optional[int] = ..., namespace: _Optional[str] = ...) -> None: ...

class Artifact(_message.Message):
    __slots__ = ("id", "type", "body", "meta", "produced_by", "derived_from", "object")
    class MetaEntry(_message.Message):
        __slots__ = ("key", "value")
        KEY_FIELD_NUMBER: _ClassVar[int]
        VALUE_FIELD_NUMBER: _ClassVar[int]
        key: str
        value: str
        def __init__(self, key: _Optional[str] = ..., value: _Optional[str] = ...) -> None: ...
    ID_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    BODY_FIELD_NUMBER: _ClassVar[int]
    META_FIELD_NUMBER: _ClassVar[int]
    PRODUCED_BY_FIELD_NUMBER: _ClassVar[int]
    DERIVED_FROM_FIELD_NUMBER: _ClassVar[int]
    OBJECT_FIELD_NUMBER: _ClassVar[int]
    id: str
    type: str
    body: bytes
    meta: _containers.ScalarMap[str, str]
    produced_by: str
    derived_from: _containers.RepeatedScalarFieldContainer[str]
    object: ObjectRef
    def __init__(self, id: _Optional[str] = ..., type: _Optional[str] = ..., body: _Optional[bytes] = ..., meta: _Optional[_Mapping[str, str]] = ..., produced_by: _Optional[str] = ..., derived_from: _Optional[_Iterable[str]] = ..., object: _Optional[_Union[ObjectRef, _Mapping]] = ...) -> None: ...

class ArtifactRef(_message.Message):
    __slots__ = ("id",)
    ID_FIELD_NUMBER: _ClassVar[int]
    id: str
    def __init__(self, id: _Optional[str] = ...) -> None: ...

class PutBlobRequest(_message.Message):
    __slots__ = ("namespace", "data")
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    DATA_FIELD_NUMBER: _ClassVar[int]
    namespace: str
    data: bytes
    def __init__(self, namespace: _Optional[str] = ..., data: _Optional[bytes] = ...) -> None: ...

class GetBlobRequest(_message.Message):
    __slots__ = ("digest", "namespace")
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    digest: str
    namespace: str
    def __init__(self, digest: _Optional[str] = ..., namespace: _Optional[str] = ...) -> None: ...

class BlobData(_message.Message):
    __slots__ = ("digest", "byte_count", "namespace", "data")
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    BYTE_COUNT_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    DATA_FIELD_NUMBER: _ClassVar[int]
    digest: str
    byte_count: int
    namespace: str
    data: bytes
    def __init__(self, digest: _Optional[str] = ..., byte_count: _Optional[int] = ..., namespace: _Optional[str] = ..., data: _Optional[bytes] = ...) -> None: ...

class HasBlobRequest(_message.Message):
    __slots__ = ("digest", "namespace")
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    NAMESPACE_FIELD_NUMBER: _ClassVar[int]
    digest: str
    namespace: str
    def __init__(self, digest: _Optional[str] = ..., namespace: _Optional[str] = ...) -> None: ...

class HasBlobResponse(_message.Message):
    __slots__ = ("exists", "byte_count")
    EXISTS_FIELD_NUMBER: _ClassVar[int]
    BYTE_COUNT_FIELD_NUMBER: _ClassVar[int]
    exists: bool
    byte_count: int
    def __init__(self, exists: bool = ..., byte_count: _Optional[int] = ...) -> None: ...

class Contract(_message.Message):
    __slots__ = ("ref", "schema", "media_type", "version", "digest", "compatible_with")
    REF_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    MEDIA_TYPE_FIELD_NUMBER: _ClassVar[int]
    VERSION_FIELD_NUMBER: _ClassVar[int]
    DIGEST_FIELD_NUMBER: _ClassVar[int]
    COMPATIBLE_WITH_FIELD_NUMBER: _ClassVar[int]
    ref: str
    schema: str
    media_type: str
    version: str
    digest: str
    compatible_with: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, ref: _Optional[str] = ..., schema: _Optional[str] = ..., media_type: _Optional[str] = ..., version: _Optional[str] = ..., digest: _Optional[str] = ..., compatible_with: _Optional[_Iterable[str]] = ...) -> None: ...

class Event(_message.Message):
    __slots__ = ("id", "topic", "type", "payload", "source", "seq", "run_id", "artifacts")
    ID_FIELD_NUMBER: _ClassVar[int]
    TOPIC_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    PAYLOAD_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    SEQ_FIELD_NUMBER: _ClassVar[int]
    RUN_ID_FIELD_NUMBER: _ClassVar[int]
    ARTIFACTS_FIELD_NUMBER: _ClassVar[int]
    id: str
    topic: str
    type: str
    payload: bytes
    source: str
    seq: int
    run_id: str
    artifacts: _containers.RepeatedCompositeFieldContainer[ArtifactRef]
    def __init__(self, id: _Optional[str] = ..., topic: _Optional[str] = ..., type: _Optional[str] = ..., payload: _Optional[bytes] = ..., source: _Optional[str] = ..., seq: _Optional[int] = ..., run_id: _Optional[str] = ..., artifacts: _Optional[_Iterable[_Union[ArtifactRef, _Mapping]]] = ...) -> None: ...

class LedgerEntry(_message.Message):
    __slots__ = ("seq", "kind", "subject", "detail", "prev_hash", "hash")
    SEQ_FIELD_NUMBER: _ClassVar[int]
    KIND_FIELD_NUMBER: _ClassVar[int]
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    DETAIL_FIELD_NUMBER: _ClassVar[int]
    PREV_HASH_FIELD_NUMBER: _ClassVar[int]
    HASH_FIELD_NUMBER: _ClassVar[int]
    seq: int
    kind: str
    subject: str
    detail: bytes
    prev_hash: str
    hash: str
    def __init__(self, seq: _Optional[int] = ..., kind: _Optional[str] = ..., subject: _Optional[str] = ..., detail: _Optional[bytes] = ..., prev_hash: _Optional[str] = ..., hash: _Optional[str] = ...) -> None: ...

class RegistrySnapshot(_message.Message):
    __slots__ = ("modules", "capabilities", "contracts")
    MODULES_FIELD_NUMBER: _ClassVar[int]
    CAPABILITIES_FIELD_NUMBER: _ClassVar[int]
    CONTRACTS_FIELD_NUMBER: _ClassVar[int]
    modules: _containers.RepeatedCompositeFieldContainer[ModuleManifest]
    capabilities: _containers.RepeatedCompositeFieldContainer[Capability]
    contracts: _containers.RepeatedCompositeFieldContainer[Contract]
    def __init__(self, modules: _Optional[_Iterable[_Union[ModuleManifest, _Mapping]]] = ..., capabilities: _Optional[_Iterable[_Union[Capability, _Mapping]]] = ..., contracts: _Optional[_Iterable[_Union[Contract, _Mapping]]] = ...) -> None: ...

class NamedArtifact(_message.Message):
    __slots__ = ("name", "artifact")
    NAME_FIELD_NUMBER: _ClassVar[int]
    ARTIFACT_FIELD_NUMBER: _ClassVar[int]
    name: str
    artifact: ArtifactRef
    def __init__(self, name: _Optional[str] = ..., artifact: _Optional[_Union[ArtifactRef, _Mapping]] = ...) -> None: ...

class AssemblyNode(_message.Message):
    __slots__ = ("id", "module", "module_version", "capability")
    ID_FIELD_NUMBER: _ClassVar[int]
    MODULE_FIELD_NUMBER: _ClassVar[int]
    MODULE_VERSION_FIELD_NUMBER: _ClassVar[int]
    CAPABILITY_FIELD_NUMBER: _ClassVar[int]
    id: str
    module: str
    module_version: str
    capability: str
    def __init__(self, id: _Optional[str] = ..., module: _Optional[str] = ..., module_version: _Optional[str] = ..., capability: _Optional[str] = ...) -> None: ...

class Binding(_message.Message):
    __slots__ = ("to_node", "to_port", "from_node", "from_port", "input")
    TO_NODE_FIELD_NUMBER: _ClassVar[int]
    TO_PORT_FIELD_NUMBER: _ClassVar[int]
    FROM_NODE_FIELD_NUMBER: _ClassVar[int]
    FROM_PORT_FIELD_NUMBER: _ClassVar[int]
    INPUT_FIELD_NUMBER: _ClassVar[int]
    to_node: str
    to_port: str
    from_node: str
    from_port: str
    input: str
    def __init__(self, to_node: _Optional[str] = ..., to_port: _Optional[str] = ..., from_node: _Optional[str] = ..., from_port: _Optional[str] = ..., input: _Optional[str] = ...) -> None: ...

class NodeOutput(_message.Message):
    __slots__ = ("node", "port")
    NODE_FIELD_NUMBER: _ClassVar[int]
    PORT_FIELD_NUMBER: _ClassVar[int]
    node: str
    port: str
    def __init__(self, node: _Optional[str] = ..., port: _Optional[str] = ...) -> None: ...

class Limits(_message.Message):
    __slots__ = ("max_steps",)
    MAX_STEPS_FIELD_NUMBER: _ClassVar[int]
    max_steps: int
    def __init__(self, max_steps: _Optional[int] = ...) -> None: ...

class Assembly(_message.Message):
    __slots__ = ("id", "nodes", "bindings", "terminal")
    ID_FIELD_NUMBER: _ClassVar[int]
    NODES_FIELD_NUMBER: _ClassVar[int]
    BINDINGS_FIELD_NUMBER: _ClassVar[int]
    TERMINAL_FIELD_NUMBER: _ClassVar[int]
    id: str
    nodes: _containers.RepeatedCompositeFieldContainer[AssemblyNode]
    bindings: _containers.RepeatedCompositeFieldContainer[Binding]
    terminal: NodeOutput
    def __init__(self, id: _Optional[str] = ..., nodes: _Optional[_Iterable[_Union[AssemblyNode, _Mapping]]] = ..., bindings: _Optional[_Iterable[_Union[Binding, _Mapping]]] = ..., terminal: _Optional[_Union[NodeOutput, _Mapping]] = ...) -> None: ...

class RunRequest(_message.Message):
    __slots__ = ("id", "assembly", "inputs", "limits")
    ID_FIELD_NUMBER: _ClassVar[int]
    ASSEMBLY_FIELD_NUMBER: _ClassVar[int]
    INPUTS_FIELD_NUMBER: _ClassVar[int]
    LIMITS_FIELD_NUMBER: _ClassVar[int]
    id: str
    assembly: Assembly
    inputs: _containers.RepeatedCompositeFieldContainer[NamedArtifact]
    limits: Limits
    def __init__(self, id: _Optional[str] = ..., assembly: _Optional[_Union[Assembly, _Mapping]] = ..., inputs: _Optional[_Iterable[_Union[NamedArtifact, _Mapping]]] = ..., limits: _Optional[_Union[Limits, _Mapping]] = ...) -> None: ...

class Run(_message.Message):
    __slots__ = ("id", "assembly", "inputs", "state", "answer", "steps", "max_steps", "reason")
    ID_FIELD_NUMBER: _ClassVar[int]
    ASSEMBLY_FIELD_NUMBER: _ClassVar[int]
    INPUTS_FIELD_NUMBER: _ClassVar[int]
    STATE_FIELD_NUMBER: _ClassVar[int]
    ANSWER_FIELD_NUMBER: _ClassVar[int]
    STEPS_FIELD_NUMBER: _ClassVar[int]
    MAX_STEPS_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    id: str
    assembly: Assembly
    inputs: _containers.RepeatedCompositeFieldContainer[NamedArtifact]
    state: RunState
    answer: ArtifactRef
    steps: int
    max_steps: int
    reason: str
    def __init__(self, id: _Optional[str] = ..., assembly: _Optional[_Union[Assembly, _Mapping]] = ..., inputs: _Optional[_Iterable[_Union[NamedArtifact, _Mapping]]] = ..., state: _Optional[_Union[RunState, str]] = ..., answer: _Optional[_Union[ArtifactRef, _Mapping]] = ..., steps: _Optional[int] = ..., max_steps: _Optional[int] = ..., reason: _Optional[str] = ...) -> None: ...

class RunRef(_message.Message):
    __slots__ = ("id",)
    ID_FIELD_NUMBER: _ClassVar[int]
    id: str
    def __init__(self, id: _Optional[str] = ...) -> None: ...

class ClaimRequest(_message.Message):
    __slots__ = ("run_id", "module")
    RUN_ID_FIELD_NUMBER: _ClassVar[int]
    MODULE_FIELD_NUMBER: _ClassVar[int]
    run_id: str
    module: str
    def __init__(self, run_id: _Optional[str] = ..., module: _Optional[str] = ...) -> None: ...

class WorkItem(_message.Message):
    __slots__ = ("id", "run_id", "node_id", "module", "module_version", "capability", "inputs")
    ID_FIELD_NUMBER: _ClassVar[int]
    RUN_ID_FIELD_NUMBER: _ClassVar[int]
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    MODULE_FIELD_NUMBER: _ClassVar[int]
    MODULE_VERSION_FIELD_NUMBER: _ClassVar[int]
    CAPABILITY_FIELD_NUMBER: _ClassVar[int]
    INPUTS_FIELD_NUMBER: _ClassVar[int]
    id: str
    run_id: str
    node_id: str
    module: str
    module_version: str
    capability: str
    inputs: _containers.RepeatedCompositeFieldContainer[NamedArtifact]
    def __init__(self, id: _Optional[str] = ..., run_id: _Optional[str] = ..., node_id: _Optional[str] = ..., module: _Optional[str] = ..., module_version: _Optional[str] = ..., capability: _Optional[str] = ..., inputs: _Optional[_Iterable[_Union[NamedArtifact, _Mapping]]] = ...) -> None: ...

class Derivation(_message.Message):
    __slots__ = ("id", "run_id", "work_id", "node_id", "module", "module_version", "capability", "inputs", "outputs")
    ID_FIELD_NUMBER: _ClassVar[int]
    RUN_ID_FIELD_NUMBER: _ClassVar[int]
    WORK_ID_FIELD_NUMBER: _ClassVar[int]
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    MODULE_FIELD_NUMBER: _ClassVar[int]
    MODULE_VERSION_FIELD_NUMBER: _ClassVar[int]
    CAPABILITY_FIELD_NUMBER: _ClassVar[int]
    INPUTS_FIELD_NUMBER: _ClassVar[int]
    OUTPUTS_FIELD_NUMBER: _ClassVar[int]
    id: str
    run_id: str
    work_id: str
    node_id: str
    module: str
    module_version: str
    capability: str
    inputs: _containers.RepeatedCompositeFieldContainer[NamedArtifact]
    outputs: _containers.RepeatedCompositeFieldContainer[NamedArtifact]
    def __init__(self, id: _Optional[str] = ..., run_id: _Optional[str] = ..., work_id: _Optional[str] = ..., node_id: _Optional[str] = ..., module: _Optional[str] = ..., module_version: _Optional[str] = ..., capability: _Optional[str] = ..., inputs: _Optional[_Iterable[_Union[NamedArtifact, _Mapping]]] = ..., outputs: _Optional[_Iterable[_Union[NamedArtifact, _Mapping]]] = ...) -> None: ...

class DerivationList(_message.Message):
    __slots__ = ("derivations",)
    DERIVATIONS_FIELD_NUMBER: _ClassVar[int]
    derivations: _containers.RepeatedCompositeFieldContainer[Derivation]
    def __init__(self, derivations: _Optional[_Iterable[_Union[Derivation, _Mapping]]] = ...) -> None: ...

class RequestContext(_message.Message):
    __slots__ = ("caller", "request_key", "deadline_unix_ms", "correlation_id")
    CALLER_FIELD_NUMBER: _ClassVar[int]
    REQUEST_KEY_FIELD_NUMBER: _ClassVar[int]
    DEADLINE_UNIX_MS_FIELD_NUMBER: _ClassVar[int]
    CORRELATION_ID_FIELD_NUMBER: _ClassVar[int]
    caller: str
    request_key: str
    deadline_unix_ms: int
    correlation_id: str
    def __init__(self, caller: _Optional[str] = ..., request_key: _Optional[str] = ..., deadline_unix_ms: _Optional[int] = ..., correlation_id: _Optional[str] = ...) -> None: ...

class Error(_message.Message):
    __slots__ = ("code", "message", "retryable", "conflict_subject", "failed_precondition")
    CODE_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    RETRYABLE_FIELD_NUMBER: _ClassVar[int]
    CONFLICT_SUBJECT_FIELD_NUMBER: _ClassVar[int]
    FAILED_PRECONDITION_FIELD_NUMBER: _ClassVar[int]
    code: ErrorCode
    message: str
    retryable: bool
    conflict_subject: str
    failed_precondition: str
    def __init__(self, code: _Optional[_Union[ErrorCode, str]] = ..., message: _Optional[str] = ..., retryable: bool = ..., conflict_subject: _Optional[str] = ..., failed_precondition: _Optional[str] = ...) -> None: ...

class RegisterAck(_message.Message):
    __slots__ = ("state",)
    STATE_FIELD_NUMBER: _ClassVar[int]
    state: Lifecycle
    def __init__(self, state: _Optional[_Union[Lifecycle, str]] = ...) -> None: ...

class PublishAck(_message.Message):
    __slots__ = ("seq",)
    SEQ_FIELD_NUMBER: _ClassVar[int]
    seq: int
    def __init__(self, seq: _Optional[int] = ...) -> None: ...

class Subscription(_message.Message):
    __slots__ = ("module", "topics")
    MODULE_FIELD_NUMBER: _ClassVar[int]
    TOPICS_FIELD_NUMBER: _ClassVar[int]
    module: str
    topics: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, module: _Optional[str] = ..., topics: _Optional[_Iterable[str]] = ...) -> None: ...

class SnapshotRequest(_message.Message):
    __slots__ = ()
    def __init__(self) -> None: ...

class AppendRequest(_message.Message):
    __slots__ = ("kind", "subject", "detail")
    KIND_FIELD_NUMBER: _ClassVar[int]
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    DETAIL_FIELD_NUMBER: _ClassVar[int]
    kind: str
    subject: str
    detail: bytes
    def __init__(self, kind: _Optional[str] = ..., subject: _Optional[str] = ..., detail: _Optional[bytes] = ...) -> None: ...

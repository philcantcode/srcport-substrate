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

class Decision(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    DECISION_UNSPECIFIED: _ClassVar[Decision]
    DECISION_PENDING: _ClassVar[Decision]
    DECISION_APPROVED: _ClassVar[Decision]
    DECISION_REJECTED: _ClassVar[Decision]

class RunState(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    RUN_STATE_UNSPECIFIED: _ClassVar[RunState]
    RUN_STATE_RUNNING: _ClassVar[RunState]
    RUN_STATE_COMPLETED: _ClassVar[RunState]
    RUN_STATE_STALLED: _ClassVar[RunState]
    RUN_STATE_FAILED: _ClassVar[RunState]
    RUN_STATE_CANCELLED: _ClassVar[RunState]
LIFECYCLE_UNSPECIFIED: Lifecycle
LIFECYCLE_REGISTERED: Lifecycle
LIFECYCLE_LOADED: Lifecycle
LIFECYCLE_ACTIVE: Lifecycle
LIFECYCLE_DEACTIVATED: Lifecycle
DECISION_UNSPECIFIED: Decision
DECISION_PENDING: Decision
DECISION_APPROVED: Decision
DECISION_REJECTED: Decision
RUN_STATE_UNSPECIFIED: RunState
RUN_STATE_RUNNING: RunState
RUN_STATE_COMPLETED: RunState
RUN_STATE_STALLED: RunState
RUN_STATE_FAILED: RunState
RUN_STATE_CANCELLED: RunState

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

class Artifact(_message.Message):
    __slots__ = ("id", "type", "body", "meta", "produced_by", "derived_from")
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
    id: str
    type: str
    body: bytes
    meta: _containers.ScalarMap[str, str]
    produced_by: str
    derived_from: _containers.RepeatedScalarFieldContainer[str]
    def __init__(self, id: _Optional[str] = ..., type: _Optional[str] = ..., body: _Optional[bytes] = ..., meta: _Optional[_Mapping[str, str]] = ..., produced_by: _Optional[str] = ..., derived_from: _Optional[_Iterable[str]] = ...) -> None: ...

class ArtifactRef(_message.Message):
    __slots__ = ("id",)
    ID_FIELD_NUMBER: _ClassVar[int]
    id: str
    def __init__(self, id: _Optional[str] = ...) -> None: ...

class Contract(_message.Message):
    __slots__ = ("ref", "schema")
    REF_FIELD_NUMBER: _ClassVar[int]
    SCHEMA_FIELD_NUMBER: _ClassVar[int]
    ref: str
    schema: str
    def __init__(self, ref: _Optional[str] = ..., schema: _Optional[str] = ...) -> None: ...

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

class GateRequest(_message.Message):
    __slots__ = ("id", "action", "context", "requested_by")
    ID_FIELD_NUMBER: _ClassVar[int]
    ACTION_FIELD_NUMBER: _ClassVar[int]
    CONTEXT_FIELD_NUMBER: _ClassVar[int]
    REQUESTED_BY_FIELD_NUMBER: _ClassVar[int]
    id: str
    action: str
    context: bytes
    requested_by: str
    def __init__(self, id: _Optional[str] = ..., action: _Optional[str] = ..., context: _Optional[bytes] = ..., requested_by: _Optional[str] = ...) -> None: ...

class GateDecision(_message.Message):
    __slots__ = ("request_id", "decision", "decided_by", "reason")
    REQUEST_ID_FIELD_NUMBER: _ClassVar[int]
    DECISION_FIELD_NUMBER: _ClassVar[int]
    DECIDED_BY_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    request_id: str
    decision: Decision
    decided_by: str
    reason: str
    def __init__(self, request_id: _Optional[str] = ..., decision: _Optional[_Union[Decision, str]] = ..., decided_by: _Optional[str] = ..., reason: _Optional[str] = ...) -> None: ...

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

class GateTicket(_message.Message):
    __slots__ = ("request_id",)
    REQUEST_ID_FIELD_NUMBER: _ClassVar[int]
    request_id: str
    def __init__(self, request_id: _Optional[str] = ...) -> None: ...

class AppendRequest(_message.Message):
    __slots__ = ("kind", "subject", "detail")
    KIND_FIELD_NUMBER: _ClassVar[int]
    SUBJECT_FIELD_NUMBER: _ClassVar[int]
    DETAIL_FIELD_NUMBER: _ClassVar[int]
    kind: str
    subject: str
    detail: bytes
    def __init__(self, kind: _Optional[str] = ..., subject: _Optional[str] = ..., detail: _Optional[bytes] = ...) -> None: ...

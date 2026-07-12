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
LIFECYCLE_UNSPECIFIED: Lifecycle
LIFECYCLE_REGISTERED: Lifecycle
LIFECYCLE_LOADED: Lifecycle
LIFECYCLE_ACTIVE: Lifecycle
LIFECYCLE_DEACTIVATED: Lifecycle
DECISION_UNSPECIFIED: Decision
DECISION_PENDING: Decision
DECISION_APPROVED: Decision
DECISION_REJECTED: Decision

class Capability(_message.Message):
    __slots__ = ("name", "contract")
    NAME_FIELD_NUMBER: _ClassVar[int]
    CONTRACT_FIELD_NUMBER: _ClassVar[int]
    name: str
    contract: str
    def __init__(self, name: _Optional[str] = ..., contract: _Optional[str] = ...) -> None: ...

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
    __slots__ = ("id", "topic", "type", "payload", "source", "seq")
    ID_FIELD_NUMBER: _ClassVar[int]
    TOPIC_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    PAYLOAD_FIELD_NUMBER: _ClassVar[int]
    SOURCE_FIELD_NUMBER: _ClassVar[int]
    SEQ_FIELD_NUMBER: _ClassVar[int]
    id: str
    topic: str
    type: str
    payload: bytes
    source: str
    seq: int
    def __init__(self, id: _Optional[str] = ..., topic: _Optional[str] = ..., type: _Optional[str] = ..., payload: _Optional[bytes] = ..., source: _Optional[str] = ..., seq: _Optional[int] = ...) -> None: ...

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

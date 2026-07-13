module github.com/philcantcode/srcport-substrate/framework/sdk/go

go 1.23

require github.com/philcantcode/srcport-substrate/kernel/sdk/go v0.0.0

require google.golang.org/protobuf v1.36.11 // indirect

replace github.com/philcantcode/srcport-substrate/kernel/sdk/go => ../../../kernel/sdk/go

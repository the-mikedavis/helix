[
  ; zig
  (ArrayTypeStart)
  ; using ()
  (AsmExpr)
  (AsmOutputItem)
  (ByteAlign)
  (CallConv)
  (ContainerDeclType)
  (ErrorSetDecl)
  (FnCallArguments)
  (ForPrefix)
  (GroupedExpr)
  (IfPrefix)
  (ParamDeclList)
  (SwitchExpr)
  (WhileContinueExpr)
  (WhilePrefix)
  ; for align expressions
  (PtrTypeStart)

  ; using {}
  (Block)
  (BlockExpr)
  (EscapeSequence)
  (FormatSequence)
  (InitList)
  
  ; using []
  (SliceTypeStart)
  (SuffixOp)
] @rainbow.scope

[
  "(" ")"
  "{" "}"
  "[" "]"
] @rainbow.bracket

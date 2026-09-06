(function_declaration name: (identifier) @name) @def.fn

(method_declaration name: (field_identifier) @name) @def.method

(type_spec name: (type_identifier) @name type: (struct_type)) @def.struct

(type_spec name: (type_identifier) @name type: (interface_type)) @def.trait

(type_spec
  name: (type_identifier) @name
  type: [(type_identifier)
         (qualified_type)
         (pointer_type)
         (map_type)
         (slice_type)
         (array_type)
         (function_type)
         (channel_type)
         (generic_type)]) @def.type

(type_alias name: (type_identifier) @name) @def.type

(field_declaration name: (field_identifier) @name) @def.field

(method_elem name: (field_identifier) @name) @def.method

(const_spec name: (identifier) @name) @def.const

(var_spec name: (identifier) @name) @def.static

(block_mapping_pair
  key: (flow_node) @name
  value: [(block_node) (flow_node (flow_sequence)) (flow_node (flow_mapping))]) @def.module

(block_mapping_pair
  key: (flow_node) @name
  value: (flow_node [(plain_scalar) (double_quote_scalar) (single_quote_scalar)])) @def.field

(block_mapping_pair key: (flow_node) @name . ) @def.field

(flow_pair key: (flow_node) @name) @def.field

(block_sequence (block_sequence_item) @name @def.module)

use std::collections::HashMap;

use tree_sitter::{Node, TreeCursor as TSTreeCursor};

use slotmap::{DefaultKey as LayerId, HopSlotMap};

type NodeId = usize;

pub struct InjectionLayer<'a> {
    pub root: Node<'a>,
    pub cursor: TSTreeCursor<'a>,
    pub children: HashMap<NodeId, LayerId>,
    pub parent: Option<(LayerId, Node<'a>)>,
}

impl<'a> InjectionLayer<'a> {
    pub fn new(root: Node<'a>, parent: Option<(LayerId, Node<'a>)>) -> Self {
        Self {
            root,
            cursor: root.walk(),
            children: HashMap::new(),
            parent,
        }
    }
}

/// A stateful object for walking over nodes across injection layers efficiently.
///
/// This type is similar to [`tree_sitter::TreeCursor`] but it works across
/// injection layers.
pub struct TreeCursor<'a> {
    pub layers: HopSlotMap<LayerId, InjectionLayer<'a>>,
    pub root: LayerId,
    pub current: LayerId,
}

// you reside on the parent layer's injection node, not the injection layer's node.
impl<'a> TreeCursor<'a> {
    pub fn node(&self) -> Node<'a> {
        self.layers[self.current].cursor.node()
    }

    pub fn field_name(&self) -> Option<&'static str> {
        self.layers[self.current].cursor.field_name()
    }

    pub fn goto_first_child(&mut self) -> bool {
        // TODO: do we reside on the injection layer's root node or the injection
        // node of the parent layer?
        // * if former, transition then descend
        // * if latter, descend then transition
        //
        // descend then transition. We never reside on the root of a subtree
        // (but we can reside on the absolute root with enough `goto_parent`
        // calls).

        if self.layers[self.current].cursor.goto_first_child() {
            // If the current layer has a child node, transition to that
            // node.
            true
        } else {
            let node_id = self.layers[self.current].cursor.node().id();
            match self.layers[self.current].children.get(&node_id) {
                // The cursor is on a node which injects a layer. Transition
                // to the child layer and then descend in that layer.
                Some(child_layer_id) => {
                    self.current = *child_layer_id;
                    let root = self.layers[self.current].root;
                    self.layers[self.current].cursor.reset(root);
                    self.goto_first_child()
                }
                // The cursor is at a leaf node in a leaf injection layer
                // and cannot descend further.
                None => false,
            }
        }
    }

    pub fn goto_next_sibling(&mut self) -> bool {
        // TODO: Does this need to change?
        self.layers[self.current].cursor.goto_next_sibling()
    }

    pub fn goto_parent(&mut self) -> bool {
        if self.layers[self.current].cursor.goto_parent() {
            // If the current layer has a parent node, transition to that
            // node.
            true
        } else if self.current == self.root {
            // If the cursor cannot ascend and the current layer is the root
            // layer, we are at the root node and cannot ascend.
            false
        } else if let Some((parent_layer, parent_node)) = self.layers[self.current].parent {
            // Transition up one layer and reset the cursor to the node in
            // that layer which injected the prior layer. Then ascend in that
            // subtree.
            self.current = parent_layer;
            self.layers[self.current].cursor.reset(parent_node);
            self.goto_parent()
        } else {
            false
        }
    }

    /// Moves the cursor so that the current node is the largest node in the tree
    /// which is contained in the given range.
    pub fn goto_byte_range(&mut self, start: usize, end: usize) {
        // Reset the cursor to the root of the root layer.
        self.current = self.root;
        let root = self.layers[self.current].root;
        self.layers[self.current].cursor.reset(root);

        loop {
            let range = self.layers[self.current].cursor.node().byte_range();

            // If the node's range is contained, we're done.
            if range.start >= start && range.end <= end {
                break;
            }

            // Otherwise, if the node's range ends before the given start, move to the
            // next sibling.
            if range.end <= start {
                if self.goto_next_sibling() {
                    continue;
                } else {
                    // The given range is past the end of the current tree.
                    // Should be unreachable?
                    unreachable!("past tree boundaries");
                    // break;
                }
            }

            // Otherwise, if the current node's range contains the byte range, descend to
            // the children.
            if range.start <= start && range.end >= end {
                if self.goto_first_child() {
                    continue;
                } else {
                    // If there aren't any children then the given range must
                    // be smaller than any containing nodes.
                    break;
                }
            }

            unreachable!("unexpected");
        }
    }
}

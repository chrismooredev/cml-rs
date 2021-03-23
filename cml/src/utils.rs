
use crate::rest_types as rt;

pub fn node_definitions_with_consoles<'a, II: IntoIterator<Item = &'a rt::SimpleNodeDefinition>>(defs: II) -> Vec<rt::NodeDefinition> {
	defs.into_iter()
		.filter(|nd| nd.data.sim.console)
		.map(|nd| nd.id.clone())
		.collect::<Vec<rt::NodeDefinition>>()
}


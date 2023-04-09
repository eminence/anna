use async_trait::async_trait;
use wasmtime::{Config, Engine, component::{Component, Linker}, Store};

wasmtime::component::bindgen!({
    world: "foo",
    async: true
});

pub struct HostImports;

#[async_trait]
impl host::Host for HostImports {
    async fn gen_random_integer(&mut self) -> anyhow::Result<u32> {
        Ok(42)
    }
}


#[tokio::test]
async fn test() -> anyhow::Result<()> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);
    let engine = Engine::new(&config)?;
    let component = Component::from_file(&engine, "./plugins/my-component.wasm")?;

    let mut linker = Linker::new(&engine);
    ChatPlugin::add_to_linker(&mut linker, |state: &mut HostImports| state)?;

    let mut store = Store::new(
        &engine,
        HostImports,
    );

    let (bindings, _) = ChatPlugin::instantiate_async(&mut store, &component, &linker).await?;

    
    let x = bindings.call_get_chat_instruction(&mut store, "!chat:temp=0.4,save=no,pastebin").await;
    
    dbg!(x);
    

    Ok(())

}
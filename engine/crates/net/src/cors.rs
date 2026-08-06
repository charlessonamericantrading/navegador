use crate::request::NetworkRequest;
use crate::response::NetworkResponse;

/// Stub honesto (ver ARCHITECTURE.md, diagrama de crates: "CORS (stub
/// honesto: permite todo)"): NO compara origenes ni lee cabeceras
/// `Access-Control-Allow-Origin` de verdad, siempre devuelve `true`. Ademas,
/// nada en el pipeline llama a `check_origin` todavia - `NetworkEngine::fetch`
/// (`http_client.rs`) no lo invoca, asi que hoy ninguna peticion pasa por
/// aqui. Sirve solo para fijar la forma de la API para cuando se implemente
/// CORS de verdad.
#[derive(Debug)]
pub struct CorsPolicy;

impl CorsPolicy {
    pub fn check_origin(_request: &NetworkRequest, _response: &NetworkResponse) -> bool {
        true
    }
}

use crate::vault::{SkillMetadata, Vault};
use anyhow::Result;
use serde_json::Value;
use std::{
    collections::HashSet,
    fmt::Write as _,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

const CATALOG_LIMIT: i64 = 30;
pub const WORK_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const REVIEW_TOOLS: [&str; 2] = ["publish_skill", "report_skill_outcome"];
// Loading a skill is work too: whoever applied it owes an outcome report. Matched by
// suffix, like the review tools, since each client prefixes MCP tool names its own way.
const LOAD_TOOL: &str = "get_skill";
// Devin tool names are lowercase and its hook payloads carry no transcript path,
// so the same worked/reviewed judgment is kept as per-session flags instead.
pub const DEVIN_WORK_TOOLS: &[&str] = &["write", "edit", "apply_patch", "notebook_edit"];
// The file-editing tool names of the other flag-tracked clients, as their
// PostToolUse-style hook payloads report them in `tool_name`.
pub const CODEX_WORK_TOOLS: &[&str] = &["apply_patch"];
pub const GEMINI_WORK_TOOLS: &[&str] = &["write_file", "replace"];
// Cursor's documented native tools are Shell, Read, Write, Grep, Delete, and Task; file
// edits (not just whole-file writes) go through the same "Write" tool, with no separate
// "Edit"/"MultiEdit" name. https://cursor.com/docs/hooks
pub const CURSOR_WORK_TOOLS: &[&str] = &["Write"];

// Keep in sync with REVIEW_REMINDER in assets/opencode/skillvolution.js (a test in
// setup/plugin.rs checks it).
pub const STOP_REASON: &str = "Skillvolution review: you edited files or loaded vault skills since the last review. \
Follow the Report and Reflect steps of the evolution skill now: first call report_skill_outcome for every skill you loaded with get_skill \
(helped, failed with the wrong step, or not_applicable); then, for any candidate lesson that meets every lesson criterion, \
dispatch a fresh subagent that follows the skill-evaluation skill now \
— do not ask the user first — to judge it (global scope, project scope, or discard), then call publish_skill with the verdict and its reason. \
If there is nothing to report or evaluate, say so in a single short line with no elaboration and no heading.";

/// Prompt retrieval limits: at most `PROMPT_TERMS` keywords of at most
/// `MAX_TERM_BYTES` each (16 * 30 bytes plus separators stays under `search`'s
/// 512-byte query cap), `PROMPT_CANDIDATES` FTS hits judged, `PROMPT_MATCHES` kept.
const PROMPT_TERMS: usize = 16;
const MAX_TERM_BYTES: usize = 30;
const PROMPT_CANDIDATES: i64 = 5;
const PROMPT_MATCHES: usize = 3;
/// A prompt needs this many keywords before it is searched at all: one word left
/// after filtering ("tests", "hola") says too little about the task to suggest a skill.
const MIN_PROMPT_TERMS: usize = 2;
/// Net failures (failed - helped) on the current version that keep a skill out of
/// prompt suggestions: search only demotes it, but an unprompted suggestion should not
/// push a procedure that keeps failing.
const MAX_NET_FAILURES: i64 = 2;
/// Net not-applicable reports (not_applicable - helped) on the current version that
/// keep a skill out of prompt suggestions: agents keep loading it for tasks it doesn't
/// fit, so its description triggers wrongly. Search ranking ignores not_applicable, so
/// an explicit search still finds it.
const MAX_NET_NOT_APPLICABLE: i64 = 2;

/// Two-letter technical terms (folded) that `keywords` keeps although shorter than 3
/// characters; they match whole words only, never as prefixes.
const SHORT_TERMS: [&str; 15] = [
    "ci", "cd", "go", "js", "ts", "ui", "ux", "db", "os", "vm", "ai", "ml", "qa", "pr", "io",
];

/// Words that carry no task meaning, English then Spanish (users prompt in both),
/// whitespace-separated, already lowercased and stripped of diacritics the way `fold`
/// leaves a token. Words shorter than 3 characters are dropped separately (bar
/// `SHORT_TERMS`), so they need no entry here. "use" and "when" matter most: every skill description starts with
/// "Use when", so either one would otherwise match the whole vault.
const STOPWORDS: &str = "\
about above after again all also and any anything are aren because been before \
being below between both but can cannot could couldn did didn does doesn doing \
don done down during each else even every everything few for from further get \
gets getting give got had has hasn have haven having hello help her here hers \
herself hey him himself his how into isn its itself just know let lets like \
look make may might more most much must myself need needs new not now off once \
one only other our ours ourselves out over own please put really same see she \
should shouldn some something still such sure take than thank thanks that the \
their theirs them themselves then there these they thing things this those \
through too try under until upon use used uses using very want wants was wasn \
way well were weren what when where which while who whom why will with within \
without won would wouldn yes yet you your yours yourself yourselves \
ahora algo alguien alguna algunas alguno algunos ante antes aqui asi aun aunque \
ayuda ayudame ayudar bien cada casi como con contra cosa cosas cual cuales \
cuando cuanto dame deberia deja dejan del desde despues dime donde dos durante \
ella ellas ellos entonces entre era eres esa esas ese eso esos esta estaba estan \
estar estas este esto estos estoy favor forma fue gracias haber hace hacer hacia \
hago hasta hay haz hola igual igualmente las les los luego mal manera mas \
mientras mio mis misma mismo monton mucha muchas mucho muchos muy nada nadie \
necesita necesito ningun ninguna ninguno nos nosotros nuestra nuestro nunca otra \
otras otro otros para parece pasa pero poco podria por porque puede puedes puedo \
pues que queda quedan quien quienes quiere quieres quiero quisiera sea segun ser \
sido siempre sigo sigue sin sino sobre solo son sus suya suyo tal tambien tan \
tanto tengo tiene tienes toda todas todavia todo todos tras tres tus tuyo una \
unas uno unos usa usar uso varias varios veces vez vosotros";

/// Lowercases `token` and strips the Latin diacritics FTS5's `remove_diacritics 2`
/// folds too, so "Cómo"/"como" and "configuración"/"configuracion" compare equal.
fn fold(token: &str) -> String {
    token
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// The search keywords in `text`: split on non-alphanumerics, folded (see `fold`),
/// dropping stopwords, tokens shorter than 3 characters (except the 2-letter
/// `SHORT_TERMS` such as "ci" or "go") or longer than `MAX_TERM_BYTES` (hashes,
/// paths), and repeats, in first-seen order, capped at `PROMPT_TERMS`. FTS matches
/// any prefix of any term of 3+ characters, so every word let through here can pull
/// in skills; this keeps a prompt's filler from flooding the results.
pub fn keywords(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    text.split(|c: char| !c.is_alphanumeric())
        .map(fold)
        .filter(|term| {
            (term.chars().count() >= 3 || SHORT_TERMS.contains(&term.as_str()))
                && term.len() <= MAX_TERM_BYTES
        })
        .filter(|term| !STOPWORDS.split_whitespace().any(|word| word == term))
        .filter(|term| seen.insert(term.clone()))
        .take(PROMPT_TERMS)
        .collect()
}

/// Developer words too common to say WHICH procedure applies ("fix the tests", "add a
/// file", "the build errors"): nearly every skill could claim them. They still go to
/// search, which weighs them by rarity, but never count toward `clears_gate`. Folded,
/// whitespace-separated, every inflection listed (no stemming here). "make", "new",
/// "get", and "use" are already stopwords.
const GENERIC: &str = "\
add adds added adding app apps build builds building built change changes changed \
changing code create creates created creating error errors file files fix fixes \
fixed fixing issue issues problem problems run runs running script scripts set sets \
test tests tested testing update updates updated updating work works worked working \
write writes writing written";

/// Search hits scoring below this fraction of the top hit are not suggested: they
/// matched a word or two of the prompt where the top hit matched its gist, and they
/// are what crowded suggestions with same-domain skills.
const RELATIVE_CUTOFF: f64 = 0.5;

/// Spanish developer vocabulary and its English equivalents, so a Spanish prompt
/// finds the (English) skills that describe its task, and a Spanish word stops
/// prefix-matching unrelated English ("genera" -> "generation"). Entries are
/// `spanish=english+english`, keys folded the way `fold` leaves a token. Only words
/// that are not themselves common English are keys (no "red", "once", "come", "pie",
/// "sin", "cola", "media", "principal"); an entry with no translation ("cargo=")
/// marks an English word that `ES_INFLECTIONS` would otherwise read as Spanish
/// ("cargo" as a form of "carga"). Each translation fits `MAX_TERM_BYTES` once its
/// `+` become spaces, so a translated prompt still fits `search`'s query cap.
const ES_EN: &str = "\
fallo=fail+failure falla=fail+failure fallar=fail errores=error roto=broken+break \
romper=break+broken rompe=break+broken caida=crash caido=crash colgado=hang+stuck \
colgar=hang cuelga=hang cuelgue=hang atascado=stuck+hang atascar=stuck \
congelado=freeze+frozen congelar=freeze bloqueo=lock+block bloquear=lock+block \
interbloqueo=deadlock lento=slow lentitud=slow rendimiento=performance fuga=leak \
aviso=warning advertencia=warning excepcion=exception traza=trace depurar=debug \
depuracion=debug arreglar=fix corregir=fix solucionar=fix reparar=fix+repair \
solucion=solution+fix problema=problem reintento=retry reintentar=retry \
agotado=timeout+exhausted caducado=expired+expire caducar=expire expirar=expire \
perder=lost+lose pierde=lost+lose perdido=lost faltar=missing falta=missing \
vacio=empty duplicado=duplicate desbordamiento=overflow caracter=character \
caracteres=character codificacion=encoding permiso=permission denegado=denied \
acceso=access inaccesible=unreachable inestable=flaky+unstable \
intermitente=intermittent+flaky aleatorio=random esperado=expected \
inesperado=unexpected desconocido=unknown invalido=invalid valido=valid \
incorrecto=wrong+incorrect erroneo=wrong equivocado=wrong nulo=null \
indefinido=undefined obsoleto=stale+obsolete desactualizado=stale+outdated viejo=old \
antiguo=old+legacy muerto=dead morir=die+dead muere=die+dead murio=die+dead \
vivo=alive huerfano=orphan senal=signal abortar=abort terminar=exit+finish \
salir=exit salida=output+exit entrada=input+entry despliegue=deploy+deployment \
desplegar=deploy+deployment despliega=deploy+deployment contenedor=container \
imagen=image servidor=server servicio=service redes=network puerto=port nodo=node \
maquina=machine nube=cloud almacenamiento=storage disco=disk memoria=memory \
registro=log+registry bitacora=log configuracion=config+configure+configuration \
configurar=config+configure+configuration entorno=environment+env clave=key \
contrasena=password secreto=secret certificado=certificate+cert \
cifrado=encryption+encrypt cifrar=encrypt firma=signature+sign firmar=sign \
usuario=user cuenta=account sesion=session autenticacion=auth+authentication \
peticion=request solicitud=request respuesta=response cabecera=header \
encabezado=header+heading enlace=link pagina=page navegador=browser \
descargar=download descarga=download subir=upload+push subida=upload \
bajar=download+pull carga=load cargar=load archivo=file fichero=file \
carpeta=folder+directory directorio=directory ruta=path+route copia=copy+backup \
respaldo=backup restaurar=restore borrar=delete+remove eliminar=delete+remove \
crear=create generar=generate instalar=install instalacion=install+installation \
desinstalar=uninstall actualizar=update+upgrade actualizacion=update+upgrade \
versiones=version dependencia=dependency paquete=package libreria=library \
biblioteca=library compilar=build+compile compilacion=build+compile \
compilador=compiler enlazador=linker enlazar=link ejecutar=run+execute \
ejecucion=run+execution ejecutable=executable+binary lanzar=launch+run \
arrancar=start+startup+boot arranque=startup+boot iniciar=start+init \
inicio=start+startup reiniciar=restart reinicio=restart parar=stop detener=stop \
detiene=stop apagar=shutdown+stop matar=kill proceso=process subproceso=subprocess \
hilo=thread tarea=task+job trabajo=job+work programado=scheduled datos=data+database \
tabla=table consulta=query indice=index esquema=schema migracion=migration \
transaccion=transaction fila=row columna=column celda=cell hoja=sheet+spreadsheet \
formato=format estilo=style fuente=font+source titulo=title+heading \
documento=document plantilla=template relleno=fill+padding rellenar=fill llenar=fill \
lleno=full ancho=width margen=margin fondo=background negrita=bold \
grafico=chart+graph ventana=window boton=button pantalla=screen escritorio=desktop \
aplicacion=app+application interfaz=interface teclado=keyboard atajo=shortcut \
complemento=plugin+extension rama=branch fusionar=merge historial=history \
reescribir=rewrite reescritura=rewrite repositorio=repo+repository clonar=clone \
etiqueta=tag+label cambio=change conflicto=conflict empujar=push sincronizar=sync \
remoto=remote revertir=revert deshacer=undo forzar=force publicar=publish+release \
lanzamiento=release+launch prueba=test probar=test unitaria=unit \
integracion=integration cobertura=coverage simular=mock+simulate simulacro=mock \
funcion=function metodo=method clase=class objeto=object tipo=type \
cadena=string+chain bucle=loop condicion=condition condicional=conditional \
valor=value parametro=parameter+param argumento=argument+arg devolver=return \
devuelve=return llamar=call llamada=call importar=import exportar=export codigo=code \
comentario=comment documentacion=docs+documentation analizar=parse+analyze \
validar=validate+validation validacion=validation+validate comprobar=check+verify \
verificar=verify+check verificacion=verify+verification comparar=compare \
buscar=search+find busqueda=search reemplazar=replace ordenar=sort \
filtrar=filter+leak filtro=filter convertir=convert serializar=serialize \
formulario=form consola=console comando=command linea=line hijo=child padre=parent \
tuberia=pipe+pipeline enviar=send envio=send recibir=receive mensaje=message \
correo=email+mail notificacion=notification evento=event gancho=hook \
disparador=trigger agente=agent subagente=subagent paralelo=parallel \
concurrencia=concurrency+concurrent simultaneo=concurrent esperar=wait espera=wait \
limite=limit rango=range bloque=block contrato=contract proveedor=provider \
balanceador=balancer inverso=reverse dominio=domain cuota=quota tasa=rate \
rastrear=crawl+scrape+trace extraer=extract+scrape sitio=site+website \
monitorizar=monitor+monitoring monitoreo=monitoring+monitor alerta=alert \
salud=health metrica=metric+metrics latencia=latency escalar=scale espejo=mirror \
limpiar=clean+cleanup limpieza=cleanup+clean purgar=purge reconstruir=rebuild \
recrear=recreate reproducir=reproduce produccion=production+prod \
desarrollo=development+dev sistema=system operativo=operating nucleo=kernel+core \
controlador=driver+controller tarjeta=card grafica=graphics+gpu \
procesador=processor+cpu plataforma=platform multiplataforma=cross+platform \
cruzada=cross estatico=static dinamico=dynamic binario=binary \
distribuir=distribute+distribution distribucion=distribution \
empaquetar=package+packaging portatil=portable+laptop instalador=installer \
demonio=daemon espacio=space inundar=flood infinito=infinite+forever \
envoltorio=wrapper vigilar=watch observar=watch+observe mostrar=show+display \
imprimir=print leer=read lectura=read escribir=write escritura=write \
guardar=save+store abrir=open cerrar=close conectar=connect+connection \
conexion=connection+connect desconectar=disconnect alcanzar=reach acceder=access \
ignorar=ignore omitir=skip+omit saltar=skip asincrono=async+asynchronous \
promesa=promise traducir=translate+translation traduccion=translation+translate \
idioma=language+locale lenguaje=language fecha=date hora=hour+time horaria=timezone \
numero=number entero=integer mayusculas=uppercase minusculas=lowercase \
propietario=owner grupo=group rol=role politica=policy regla=rule seguridad=security \
vulnerabilidad=vulnerability ocultar=hide credencial=credential+credentials \
huella=fingerprint comprimir=compress descomprimir=decompress+extract \
dividir=split+chunk trocear=chunk+split fragmento=chunk+fragment trozo=chunk \
paginacion=pagination lote=batch flujo=stream+flow renderizar=render \
dibujar=draw+render desplazar=scroll arrastrar=drag pestana=tab oscuro=dark \
tema=theme diseno=design+layout maquetacion=layout componente=component \
estado=state+status propiedad=property atributo=attribute enrutador=router \
manejador=handler intermediario=proxy escuchar=listen vincular=bind+link \
asignar=assign+allocate liberar=release+free volcado=dump compartir=share+shared \
compartido=shared montar=mount volumen=volume simbolico=symlink+symbolic \
heredado=legacy+inherited orquestador=orchestrator+orchestration \
orquestar=orchestrate orquestacion=orchestration modelo=model entrenar=train \
evaluar=evaluate+eval medir=measure puntuacion=score clasificar=classify+rank \
indexar=index sugerencia=suggestion habilidad=skill boveda=vault herramienta=tool \
marco=framework+frame requisito=requirement ajuste=setting+settings opcion=option \
bandera=flag predeterminado=default anidado=nested recursivo=recursive \
seguimiento=tracking+trace pila=stack puntero=pointer referencia=reference \
prestamo=borrow estructura=struct+structure cierre=closure+close iterador=iterator \
sobrescribir=overwrite+override anular=override+cancel heredar=inherit \
comprimido=compressed cuello=bottleneck botella=bottleneck pare=stop funcionar=work \
paso=step cargo= reproduce= divide=";

/// Spanish endings `translate` strips to find an inflected word's `ES_EN` entry, as
/// (ending, replacement) pairs tried in order: verb forms back to the infinitive
/// ("fallan" -> "fallar", "reiniciando" -> "reiniciar"), plurals back to the
/// singular ("contenedores" -> "contenedor", "pruebas" -> "prueba"), and feminine
/// back to masculine ("vieja" -> "viejo"). Never applied to a word without an entry,
/// and the stem must keep 3+ characters. Plurals only strip `as`/`os`/`es` and verbs
/// only restore `ar`/`er`/`ir`, so English words ("files", "fallen", "compile") don't
/// land on Spanish entries.
const ES_INFLECTIONS: [(&str, &str); 36] = [
    ("iendo", "er"),
    ("iendo", "ir"),
    ("ieron", "er"),
    ("ieron", "ir"),
    ("ando", "ar"),
    ("aron", "ar"),
    ("amos", "ar"),
    ("emos", "er"),
    ("imos", "ir"),
    ("ados", "ar"),
    ("adas", "ar"),
    ("idos", "er"),
    ("idos", "ir"),
    ("idas", "er"),
    ("idas", "ir"),
    ("ado", "ar"),
    ("ada", "ar"),
    ("ido", "er"),
    ("ido", "ir"),
    ("ida", "er"),
    ("ida", "ir"),
    ("an", "ar"),
    ("en", "er"),
    ("en", "ir"),
    ("as", "a"),
    ("os", "o"),
    ("as", "o"),
    ("es", "e"),
    ("es", ""),
    ("a", "ar"),
    ("a", "o"),
    ("e", "er"),
    ("e", "ir"),
    ("o", "ar"),
    ("o", "er"),
    ("o", "ir"),
];

/// The English translations of the folded Spanish `word`, from its `ES_EN` entry or,
/// failing that, the entry of the first form `ES_INFLECTIONS` reduces it to. `None`
/// for a word with no entry (English, a name, untranslated Spanish) or an entry
/// without a translation.
fn translate(word: &str) -> Option<Vec<&'static str>> {
    let entry = |key: &str| {
        ES_EN
            .split_whitespace()
            .find_map(|entry| entry.split_once('=').filter(|(es, _)| *es == key))
            .map(|(_, en)| en)
    };
    let english = entry(word).or_else(|| {
        ES_INFLECTIONS.iter().find_map(|(ending, replacement)| {
            let stem = word.strip_suffix(ending)?;
            (stem.chars().count() >= 3)
                .then(|| entry(&format!("{stem}{replacement}")))
                .flatten()
        })
    })?;
    (!english.is_empty()).then(|| english.split('+').collect())
}

/// One prompt keyword as `clears_gate` counts it: the folded word, or the English
/// translations of a Spanish word in `ES_EN`, all standing for the one keyword.
struct Keyword {
    forms: Vec<String>,
    translated: bool,
}

impl Keyword {
    fn new(word: String) -> Self {
        match translate(&word) {
            Some(english) => Keyword {
                forms: english.into_iter().map(String::from).collect(),
                translated: true,
            },
            None => Keyword {
                forms: vec![word],
                translated: false,
            },
        }
    }

    /// Whether a non-generic form matches the folded `word`. A prompt word matches the
    /// way FTS matches its `"term"*` expression: as a prefix, or as the whole word
    /// when shorter than 3 characters. A translation must be the whole word, give or
    /// take a plural or -ing/-ed ending: the dictionary chose the exact word, and a
    /// prefix would let "fail" match "failover".
    fn matches(&self, word: &str) -> bool {
        self.forms
            .iter()
            .filter(|form| !GENERIC.split_whitespace().any(|generic| generic == *form))
            .any(|form| {
                if self.translated {
                    stem(word) == stem(form)
                } else if form.chars().count() < 3 {
                    word == form
                } else {
                    word.starts_with(form.as_str())
                }
            })
    }
}

impl Keyword {
    /// Whether two keywords name one concept: a form of one equals a form of the other
    /// up to `stem`, or is a 5+ character prefix of it ("deploy" / "deployment").
    // ponytail: prefix heuristic; "config" also folds "configmap" into one keyword,
    // which only makes suggestions stricter. Use a real stemmer if that costs hits.
    fn same_concept(&self, other: &Keyword) -> bool {
        self.forms.iter().any(|a| {
            other.forms.iter().any(|b| {
                let (a, b) = (stem(a), stem(b));
                let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
                short == long || (short.len() >= 5 && long.starts_with(short))
            })
        })
    }
}

/// `word` without one trailing "s", "ing", or "ed".
// ponytail: naive English stemmer, good enough to equate translations with tag and
// description word forms; swap for a real stemmer if suggestions start missing
// obvious inflections.
fn stem(word: &str) -> &str {
    ["ing", "ed", "s"]
        .iter()
        .find_map(|suffix| word.strip_suffix(suffix).filter(|rest| rest.len() >= 3))
        .unwrap_or(word)
}

/// `text` split into folded words.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(fold)
        .collect()
}

/// Whether a search hit is relevant enough to suggest unprompted. bm25 depends on the
/// whole corpus, so instead of a score threshold this re-checks the hit's own metadata:
/// at least two distinct non-generic keywords must match its id, tags, or
/// description, in at least two different places. A place is the whole id, one whole
/// tag, or one distinct whitespace-separated description token, so "docker compose"
/// matching only the tag `docker-compose` (or only a hyphenated id, or only
/// "ipfs-http-client", in a tag and the description alike) counts once: it names one
/// thing. A hit that matched only in its body, or only on `GENERIC` words, never
/// clears. A skill whose current version has `MAX_NET_FAILURES` or more net failures,
/// or `MAX_NET_NOT_APPLICABLE` or more net not-applicable reports, is never suggested.
/// Both use current-version counts only, so a fixed version gets a fresh chance.
fn clears_gate(keywords: &[Keyword], skill: &SkillMetadata) -> bool {
    if skill.failed - skill.helped >= MAX_NET_FAILURES
        || skill.not_applicable - skill.helped >= MAX_NET_NOT_APPLICABLE
    {
        return false;
    }
    // The same compound in a tag and in the description is still one place.
    let mut seen = HashSet::new();
    let places: Vec<Vec<String>> = std::iter::once(skill.id.as_str())
        .chain(skill.tags.iter().map(String::as_str))
        .chain(skill.description.split_whitespace())
        .map(words)
        .filter(|place| !place.is_empty() && seen.insert(place.clone()))
        .collect();
    let mut matched_keywords: Vec<&Keyword> = Vec::new();
    let mut matched_places = HashSet::new();
    for keyword in keywords {
        let hits: Vec<usize> = (0..places.len())
            .filter(|&place| places[place].iter().any(|word| keyword.matches(word)))
            .collect();
        if !hits.is_empty() {
            // "deploy deployment" or "reiniciar reinicio" is one concept, one keyword.
            if !matched_keywords
                .iter()
                .any(|seen| seen.same_concept(keyword))
            {
                matched_keywords.push(keyword);
            }
            matched_places.extend(hits);
        }
    }
    let matched_keywords = matched_keywords.len();
    // Two matched keywords and two matched places always contain a pair of keywords
    // matching in different places: two keywords confined to one place share it.
    matched_keywords >= 2 && matched_places.len() >= 2
}

/// The skills worth suggesting for `prompt`: the top `PROMPT_CANDIDATES` search hits
/// for its keywords (Spanish ones translated through `ES_EN`), in search order, that
/// score at least `RELATIVE_CUTOFF` of the top hit and clear `clears_gate`, at most
/// `PROMPT_MATCHES` of them. Empty for a prompt with fewer than `MIN_PROMPT_TERMS`
/// keywords.
pub fn relevant_skills(
    vault: &Vault,
    project: Option<&str>,
    prompt: &str,
) -> Result<Vec<SkillMetadata>> {
    let terms = keywords(prompt);
    if terms.len() < MIN_PROMPT_TERMS {
        return Ok(Vec::new());
    }
    let keywords: Vec<Keyword> = terms.into_iter().map(Keyword::new).collect();
    let mut seen = HashSet::new();
    let query: Vec<&str> = keywords
        .iter()
        .flat_map(|keyword| &keyword.forms)
        .map(String::as_str)
        .filter(|form| seen.insert(*form))
        .collect();
    let page = vault.search(&query.join(" "), project, PROMPT_CANDIDATES, 0)?;
    // The cutoff is relative to the top search hit even when that hit fails the gate:
    // measured against only the hits that pass, it admitted wrong suggestions.
    let top = page.skills.first().map_or(0.0, |skill| skill.score);
    Ok(page
        .skills
        .into_iter()
        .filter(|skill| skill.score >= RELATIVE_CUTOFF * top)
        .filter(|skill| clears_gate(&keywords, skill))
        .take(PROMPT_MATCHES)
        .collect())
}

/// Prompt-submit hook: the context naming the vault skills that match the prompt in
/// the `prompt` field of `input` (the field Claude Code, Codex, Devin, and Gemini CLI
/// all use), or `None` when nothing is relevant, so the client adds nothing.
pub fn prompt(vault: &Vault, project: Option<&str>, input: &str) -> Result<Option<String>> {
    let input: Value = serde_json::from_str(input)?;
    let Some(prompt) = input["prompt"].as_str() else {
        return Ok(None);
    };
    let skills = relevant_skills(vault, project, prompt)?;
    if skills.is_empty() {
        return Ok(None);
    }
    let mut text = String::from("Skillvolution: vault skills matching this request:\n");
    for skill in &skills {
        writeln!(
            text,
            "- {} v{}: {} [helped {}, failed {}]",
            skill.id, skill.version, skill.description, skill.helped, skill.failed
        )?;
    }
    text.push_str(
        "Before starting, call get_skill for the best match and tell the user in one line \
         which vault skill you are applying (id and version), or that none fits. After \
         applying it, call report_skill_outcome.",
    );
    Ok(Some(text))
}

pub fn session_start(vault: &Vault, project: Option<&str>) -> Result<String> {
    let page = vault.search("", project, CATALOG_LIMIT, 0)?;
    let mut text = String::from(
        "Skillvolution vault (shared procedural memory). Before any non-trivial task, follow \
         step 1 of the `evolution` skill: check this catalog (run `search_skills` if nothing \
         here fits; matching skills are also suggested with each request), then tell the user \
         in one line which vault skill you are applying (id and version) or that none \
         matched. When you realize mid-task that you lack the know-how, follow the \
         `skill-acquisition` skill.\n",
    );
    if page.skills.is_empty() {
        text.push_str("No published skills are visible to this project yet.\n");
        return Ok(text);
    }
    writeln!(text, "Published skills ({} visible):", page.total)?;
    for skill in &page.skills {
        write!(
            text,
            "- {} v{}: {}",
            skill.id, skill.version, skill.description
        )?;
        if skill.helped + skill.failed > 0 {
            write!(text, " [helped {}, failed {}]", skill.helped, skill.failed)?;
        }
        text.push('\n');
    }
    let shown = page.skills.len() as i64;
    if page.total > shown {
        writeln!(
            text,
            "- ...and {} more; use search_skills",
            page.total - shown
        )?;
    }
    Ok(text)
}

/// Returns a reason to block the stop when the transcript shows work not
/// followed by an outcome report or proposal. Each transcript span is judged
/// once, and a stop already continued by this hook is never blocked.
pub fn stop(vault: &Vault, input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    let stop_hook_active = input["stop_hook_active"].as_bool() == Some(true);
    let (Some(session), Some(path)) = (
        input["session_id"].as_str(),
        input["transcript_path"].as_str(),
    ) else {
        return Ok(None);
    };
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut offset = vault.transcript_offset(session)?;
    if offset > file.metadata()?.len() {
        offset = 0;
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    // Advance the offset even when this stop was itself caused by our own
    // block (stop_hook_active): otherwise the lines Claude wrote in response
    // (e.g. a report_skill_outcome call) get re-read on the next real stop,
    // mixed in with new unreviewed work, and mask it as already reviewed.
    vault.set_transcript_offset(session, offset + complete as u64)?;
    if stop_hook_active {
        return Ok(None);
    }
    Ok(has_unreviewed_work(&bytes[..complete]).then_some(STOP_REASON))
}

/// Whether the transcript lines in `span` contain a work tool call after the
/// last review tool call. Work is a file edit or a skill load (`get_skill`), whose
/// outcome is owed. A review or load whose tool_result is an error (a rejected
/// publish, an unknown skill id) did nothing, so it does not count.
fn has_unreviewed_work(span: &[u8]) -> bool {
    let mut position = 0;
    let mut last_edit = None;
    let mut loads = Vec::new();
    let mut reviews = Vec::new();
    let mut failed = HashSet::new();
    for line in span.split(|b| *b == b'\n') {
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        visit_objects(&entry, &mut |object| match object["type"].as_str() {
            Some("tool_use") => {
                position += 1;
                let name = object["name"].as_str().unwrap_or_default();
                let id = object["id"].as_str().unwrap_or_default();
                if WORK_TOOLS.contains(&name) {
                    last_edit = Some(position);
                } else if is_load_tool(name) {
                    loads.push((position, id.to_owned()));
                } else if is_review_tool(name) {
                    reviews.push((position, id.to_owned()));
                }
            }
            Some("tool_result") if object["is_error"].as_bool() == Some(true) => {
                if let Some(id) = object["tool_use_id"].as_str() {
                    failed.insert(id.to_owned());
                }
            }
            _ => {}
        });
    }
    let last_success = |calls: Vec<(usize, String)>| {
        calls
            .into_iter()
            .filter(|(_, id)| !failed.contains(id))
            .map(|(position, _)| position)
            .max()
    };
    let last_work = last_edit.max(last_success(loads));
    let last_review = last_success(reviews);
    // None orders below any Some, so no work never blocks and work with no
    // successful review always does.
    last_work > last_review
}

fn is_review_tool(name: &str) -> bool {
    REVIEW_TOOLS.iter().any(|tool| name.ends_with(tool))
}

fn is_load_tool(name: &str) -> bool {
    name.ends_with(LOAD_TOOL)
}

/// Devin SessionStart: same catalog as `session_start`, wrapped in the JSON
/// envelope Devin expects for injected context.
pub fn devin_session_start(vault: &Vault, project: Option<&str>) -> Result<String> {
    let context = session_start(vault, project)?;
    Ok(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    })
    .to_string())
}

/// The session a hook payload belongs to: `session_id`, or `conversation_id`
/// for clients that name it that way.
fn session_of(input: &Value) -> Option<&str> {
    input["session_id"]
        .as_str()
        .or_else(|| input["conversation_id"].as_str())
}

/// Devin PostToolUse: `tool_use` with Devin's file-editing tools.
pub fn devin_tool_use(vault: &Vault, input: &str) -> Result<()> {
    tool_use(vault, input, DEVIN_WORK_TOOLS)
}

/// PostToolUse for the flag-tracked clients (every client but Claude Code, whose
/// transcript is scanned instead): flags the session as having done work (which a
/// previous review no longer covers) when the tool is one of `work_tools` or the
/// vault's `get_skill`, or as having reviewed it after a review tool. Any other tool
/// leaves the flags alone. The flags live in `devin_hook_state`, whatever the client.
pub fn tool_use(vault: &Vault, input: &str, work_tools: &[&str]) -> Result<()> {
    let input: Value = serde_json::from_str(input)?;
    let (Some(session), Some(tool)) = (session_of(&input), input["tool_name"].as_str()) else {
        return Ok(());
    };
    if is_review_tool(tool) {
        vault.mark_devin_review(session)
    } else if work_tools.contains(&tool) || is_load_tool(tool) {
        vault.mark_devin_work(session)
    } else {
        Ok(())
    }
}

/// Devin Stop: returns the review reason when the span since the last stop
/// has work not followed by a review. Every judged stop consumes its span, so
/// a stop caused by the agent simply ignoring the reminder is not blocked
/// again. A stop already continued by this hook (stop_hook_active) is never
/// blocked and leaves the flags alone.
/// The same judgment serves every flag-tracked client.
pub fn devin_stop(vault: &Vault, input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(None);
    }
    let Some(session) = session_of(&input) else {
        return Ok(None);
    };
    let (worked, reviewed) = vault.take_devin_hook_state(session)?;
    Ok((worked && !reviewed).then_some(STOP_REASON))
}

/// Devin SessionEnd: drops the session's flag row so the table does not grow
/// with dead sessions.
pub fn devin_session_end(vault: &Vault, input: &str) -> Result<()> {
    let input: Value = serde_json::from_str(input)?;
    if let Some(session) = session_of(&input) {
        vault.clear_devin_hook_state(session)?;
    }
    Ok(())
}

/// Devin tool names the evolution workflow needs without prompting: subagent
/// dispatch (not covered by the documented `permissions` tool list, which only
/// names read/edit/grep/glob/exec) is handled here via PermissionRequest.
const DEVIN_AUTO_APPROVE_TOOLS: [&str; 2] = ["run_subagent", "read_subagent"];

/// Devin PermissionRequest: approves the subagent and vault MCP tools the
/// evolution flow requires; any other tool prints nothing so the normal
/// permission prompt still runs.
pub fn devin_approve(input: &str) -> Result<Option<&'static str>> {
    let input: Value = serde_json::from_str(input)?;
    let Some(tool) = input["tool_name"].as_str() else {
        return Ok(None);
    };
    let approved =
        DEVIN_AUTO_APPROVE_TOOLS.contains(&tool) || tool.starts_with("mcp__skillvolution__");
    Ok(approved.then_some("Skillvolution-managed tool"))
}

/// Calls `visit` on every object in the value, walking all of it rather than a
/// fixed path: transcript entry shapes vary across Claude Code versions, and
/// tool_use/tool_result blocks can be nested inside content arrays at
/// different depths. Array items are visited in order, so the blocks of one
/// content array keep their transcript order.
fn visit_objects(value: &Value, visit: &mut impl FnMut(&Value)) {
    match value {
        Value::Object(map) => {
            visit(value);
            map.values().for_each(|child| visit_objects(child, visit));
        }
        Value::Array(items) => items.iter().for_each(|child| visit_objects(child, visit)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_entries_are_well_formed_and_fit_the_query_cap() {
        let mut keys = HashSet::new();
        for entry in ES_EN.split_whitespace() {
            let (spanish, english) = entry.split_once('=').expect(entry);
            assert!(
                spanish.len() >= 3 && spanish.chars().all(|c| c.is_ascii_lowercase()),
                "{entry}: keys are folded words keywords() can produce"
            );
            assert!(keys.insert(spanish), "{spanish} is listed twice");
            assert!(
                !STOPWORDS.split_whitespace().any(|word| word == spanish),
                "{spanish} is a stopword, so its entry is never reached"
            );
            assert!(
                english.replace('+', " ").len() <= MAX_TERM_BYTES,
                "{entry}: a translation must fit where its keyword did"
            );
            assert!(
                english.is_empty() || english.split('+').all(|word| !word.is_empty()),
                "{entry}"
            );
        }
    }

    #[test]
    fn inflected_spanish_words_find_their_entry() {
        for (word, english) in [
            ("fallo", vec!["fail", "failure"]),
            ("fallan", vec!["fail"]),
            ("contenedores", vec!["container"]),
            ("reiniciando", vec!["restart"]),
            ("pruebas", vec!["test"]),
            ("vieja", vec!["old"]),
            ("bloqueada", vec!["lock", "block"]),
            ("desplegaron", vec!["deploy", "deployment"]),
        ] {
            assert_eq!(translate(word), Some(english), "{word}");
        }
    }

    #[test]
    fn english_words_and_cross_language_homographs_are_never_translated() {
        // Spanish words that are common English words are left out of the
        // dictionary; English words an inflection rule could read as Spanish
        // ("cargo" as a form of "carga") are pinned untranslated.
        for word in [
            "red",
            "once",
            "come",
            "pie",
            "sin",
            "cola",
            "media",
            "principal",
            "cargo",
            "cargos",
            "files",
            "fallen",
            "compile",
            "configure",
            "table",
            "error",
            "tests",
            "deploy",
            "container",
        ] {
            assert_eq!(translate(word), None, "{word}");
        }
    }

    #[test]
    fn a_spanish_word_with_two_translations_is_one_keyword() {
        let keyword = Keyword::new("fallo".to_owned());
        assert_eq!(keyword.forms, ["fail", "failure"]);
        assert!(keyword.translated);
        assert!(keyword.matches("fails") && keyword.matches("failure"));
        assert!(
            !keyword.matches("failover"),
            "translations match whole words"
        );
        let generic = Keyword::new("arreglar".to_owned());
        assert_eq!(generic.forms, ["fix"]);
        assert!(!generic.matches("fix"), "generic words never count");
    }
}

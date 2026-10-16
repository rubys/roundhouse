//! Validation for the deliberately small generated-column expression
//! subsets that Roundhouse can render to SQLite and PostgreSQL.
//!
//! Keeping the original SQL on [`super::GeneratedColumn`] is important for
//! schema round-trips. This parser is only a gate: the default portable
//! subset accepts string literals, references to ordinary text/string
//! columns, `||`, and `coalesce(...)`. An explicit PostgreSQL DDL path also
//! accepts bounded text and int4 casts. It does not rewrite expressions or
//! pass through arbitrary functions or database-specific SQL. The PostgreSQL
//! DDL path also admits literal-key and int4-index JSON extraction from exact
//! `json` and `jsonb` source columns; the original SQL is still emitted.

use super::{Column, ColumnType, Table};

/// Expression syntax admitted while ingesting generated-column schema
/// metadata. This is a source-expression validation mode, not a project
/// database selector. Application ingest uses [`Portable`](Self::Portable)
/// so PostgreSQL-only syntax remains unsupported by current targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GeneratedExpressionDialect {
    /// The existing expression grammar shared by SQLite and PostgreSQL DDL.
    #[default]
    Portable,
    /// The portable grammar plus bounded text/int4 casts and JSON extraction
    /// used by the PostgreSQL schema-DDL renderer.
    Postgres,
}

#[derive(Clone, Debug)]
enum Expr {
    String(String),
    Column { name: String, quoted: bool },
    Concat(Box<Expr>, Box<Expr>),
    Coalesce(Vec<Expr>),
    TextCast(Box<Expr>),
    Int4Cast(Box<Expr>),
    JsonExtract {
        receiver: Box<Expr>,
        operator: JsonExtractOperator,
    },
}

#[derive(Clone, Copy, Debug)]
enum JsonExtractOperator {
    Json,
    Text,
}

#[derive(Clone, Copy, Debug)]
enum CastType {
    Text,
    Int4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueType {
    Text,
    Json,
    Jsonb,
    Int4,
}

/// Check a generated column using an explicit source-expression grammar.
pub(crate) fn validate_column_with_dialect(
    table: &Table,
    column: &Column,
    dialect: GeneratedExpressionDialect,
) -> Result<(), String> {
    let Some(generated) = &column.generated else {
        return Ok(());
    };

    if column.primary_key {
        return Err("generated columns cannot be primary keys".into());
    }
    if ["id", "created_at", "updated_at"]
        .iter()
        .any(|reserved| column.name.as_str().eq_ignore_ascii_case(reserved))
    {
        return Err(format!(
            "generated column name `{}` conflicts with Roundhouse key/timestamp handling",
            column.name.as_str()
        ));
    }
    if column.default.is_some() {
        return Err("generated columns cannot also have a default".into());
    }
    let text_result = is_text_type(&column.col_type);
    let int4_result = matches!(&column.col_type, ColumnType::Integer);
    if text_result && column.generated_text_compatible == Some(false) {
        return Err(
            "the original generated-column result type does not have portable text semantics"
                .into(),
        );
    }
    if int4_result
        && dialect == GeneratedExpressionDialect::Postgres
        && column.generated_int4_compatible == Some(false)
    {
        return Err(
            "the original generated-column result type is not an exact PostgreSQL int4"
                .into(),
        );
    }
    if !text_result && (!int4_result || dialect != GeneratedExpressionDialect::Postgres) {
        return Err(
            "the supported generated-column result types are unbounded string and text".into(),
        );
    }

    let expression = Parser::new(&generated.expression, dialect).parse()?;
    match validate_expr(&expression, table)? {
        ValueType::Text if text_result => Ok(()),
        ValueType::Int4 if int4_result => Ok(()),
        ValueType::Json | ValueType::Jsonb => Err(
            "generated JSON expressions must use `->>` at the end to produce text".into(),
        ),
        ValueType::Text => Err(
            "PostgreSQL integer generated columns require an `integer` or `int4` cast of text"
                .into(),
        ),
        ValueType::Int4 => Err(
            "PostgreSQL int4 generated expressions require an integer result column".into(),
        ),
    }
}

/// Check every generated output under one source-expression grammar.
pub(crate) fn validate_table_with_dialect(
    table: &Table,
    dialect: GeneratedExpressionDialect,
) -> Vec<(String, String)> {
    let mut errors: Vec<(String, String)> = table
        .columns
        .iter()
        .filter(|column| column.generated.is_some())
        .filter_map(|column| {
            validate_column_with_dialect(table, column, dialect)
                .err()
                .map(|message| (column.name.as_str().to_string(), message))
        })
        .collect();
    if table
        .columns
        .iter()
        .any(|column| column.generated.is_some())
        && table
            .columns
            .iter()
            .all(|column| column.generated.is_some())
    {
        errors.extend(table.columns.iter().map(|column| {
            (
                column.name.as_str().to_string(),
                "tables with generated columns require at least one ordinary column".into(),
            )
        }));
    }
    errors
}

fn is_text_type(col_type: &ColumnType) -> bool {
    matches!(
        col_type,
        ColumnType::Text | ColumnType::String { limit: None }
    )
}

/// Validate operand provenance and infer text, JSON, JSONB, or int4 so extraction chains and casts require exact input categories.
fn validate_expr(expr: &Expr, table: &Table) -> Result<ValueType, String> {
    match expr {
        Expr::String(_) => Ok(ValueType::Text),
        Expr::Column { name, quoted } => {
            let resolved = if *quoted {
                name.clone()
            } else {
                name.to_ascii_lowercase()
            };
            let Some(column) = table
                .columns
                .iter()
                .find(|column| column.name.as_str() == resolved)
            else {
                return Err(format!("expression references unknown column `{resolved}`"));
            };
            if column.generated.is_some() {
                return Err(format!(
                    "expression references generated column `{resolved}`"
                ));
            }
            if column.generated_text_compatible == Some(false) {
                return Err(format!(
                    "expression column `{resolved}` has non-portable text semantics from its original database type"
                ));
            }
            match &column.col_type {
                ColumnType::Text | ColumnType::String { limit: None } => Ok(ValueType::Text),
                ColumnType::Json => Ok(ValueType::Json),
                ColumnType::Jsonb => Ok(ValueType::Jsonb),
                other => Err(format!(
                    "expression column `{resolved}` has type {other:?}; only text/string or JSON/JSONB operands are supported"
                )),
            }
        }
        Expr::Concat(left, right) => {
            let left_type = validate_expr(left, table)?;
            let right_type = validate_expr(right, table)?;
            if left_type == ValueType::Text && right_type == ValueType::Text {
                Ok(ValueType::Text)
            } else {
                Err("`||` in generated expressions requires text/string operands".into())
            }
        }
        Expr::Coalesce(args) => {
            for arg in args {
                if validate_expr(arg, table)? != ValueType::Text {
                    return Err(
                        "coalesce in generated expressions requires text/string arguments".into(),
                    );
                }
            }
            Ok(ValueType::Text)
        }
        Expr::TextCast(expression) => {
            if validate_expr(expression, table)? != ValueType::Text {
                return Err(
                    "PostgreSQL text casts in generated expressions require a text/string operand"
                        .into(),
                );
            }
            Ok(ValueType::Text)
        }
        Expr::Int4Cast(expression) => {
            if validate_expr(expression, table)? != ValueType::Text {
                return Err(
                    "PostgreSQL int4 casts in generated expressions require a text/string operand"
                        .into(),
                );
            }
            Ok(ValueType::Int4)
        }
        Expr::JsonExtract {
            receiver,
            operator,
        } => match validate_expr(receiver, table)? {
            ValueType::Json => match operator {
                JsonExtractOperator::Json => Ok(ValueType::Json),
                JsonExtractOperator::Text => Ok(ValueType::Text),
            },
            ValueType::Jsonb => match operator {
                JsonExtractOperator::Json => Ok(ValueType::Jsonb),
                JsonExtractOperator::Text => Ok(ValueType::Text),
            },
            ValueType::Text | ValueType::Int4 => Err(
                "PostgreSQL JSON extraction requires a `json` or `jsonb` source column".into(),
            ),
        },
    }
}

struct Parser<'a> {
    source: &'a str,
    position: usize,
    dialect: GeneratedExpressionDialect,
}

impl<'a> Parser<'a> {
    /// Create a parser whose explicit dialect controls admission of PostgreSQL-only cast syntax.
    fn new(source: &'a str, dialect: GeneratedExpressionDialect) -> Self {
        Self {
            source,
            position: 0,
            dialect,
        }
    }

    fn parse(mut self) -> Result<Expr, String> {
        if self.source.contains('\0') {
            return Err("NUL bytes in generated expressions are unsupported".into());
        }
        // `structure.sql` is currently decoded with `from_utf8_lossy`
        // before its statement parser runs. Reject its replacement rune
        // so an invalid source byte cannot be retained as if it were an
        // intentional character in a string literal or identifier.
        if self.source.contains('\u{fffd}') {
            return Err(
                "replacement character U+FFFD in generated expressions is unsupported".into(),
            );
        }
        let expression = self.parse_operator_chain()?;
        self.skip_space();
        if self.position != self.source.len() {
            return Err(self.unsupported_at("unsupported SQL syntax"));
        }
        Ok(expression)
    }

    /// PostgreSQL puts `->`, `->>`, and `||` in the same left-associative
    /// generic-operator tier. Casts remain tighter because parse_postfix
    /// consumes them before this loop.
    fn parse_operator_chain(&mut self) -> Result<Expr, String> {
        let mut expression = self.parse_postfix()?;
        loop {
            self.skip_space();
            if self.source[self.position..].starts_with("->>") {
                if self.dialect != GeneratedExpressionDialect::Postgres {
                    return Err(self.unsupported_at("PostgreSQL JSON operators are unsupported"));
                }
                self.position += 3;
                self.parse_json_selector()?;
                expression = Expr::JsonExtract {
                    receiver: Box::new(expression),
                    operator: JsonExtractOperator::Text,
                };
            } else if self.source[self.position..].starts_with("->") {
                if self.dialect != GeneratedExpressionDialect::Postgres {
                    return Err(self.unsupported_at("PostgreSQL JSON operators are unsupported"));
                }
                self.position += 2;
                self.parse_json_selector()?;
                expression = Expr::JsonExtract {
                    receiver: Box::new(expression),
                    operator: JsonExtractOperator::Json,
                };
            } else if self.consume("||") {
                let right = self.parse_postfix()?;
                expression = Expr::Concat(Box::new(expression), Box::new(right));
            } else {
                return Ok(expression);
            }
        }
    }

    /// Accept only a literal string key, optionally text-cast, or a signed decimal int4 index.
    fn parse_json_selector(&mut self) -> Result<(), String> {
        self.skip_space();
        if self.peek_char().is_some_and(|ch| ch.is_ascii_digit() || ch == '-') {
            self.parse_json_index()?;
            return Ok(());
        }

        // The only text selectors admitted are SQL string literals, optionally
        // cast to an already-supported text type. This accepts Rails' common
        // `'key'::text` dump form while rejecting dynamic columns/functions.
        let expression = self.parse_postfix()?;
        /// Unwrap supported text casts only when the selector AST still contains a string literal.
        fn string_literal(expression: Expr) -> Option<String> {
            match expression {
                Expr::String(value) => Some(value),
                Expr::TextCast(inner) => string_literal(*inner),
                _ => None,
            }
        }
        if string_literal(expression).is_some() {
            Ok(())
        } else {
            Err(
                "PostgreSQL JSON extraction selectors must be a string literal or signed int4 literal".into()
            )
        }
    }

    /// Parse a signed decimal selector within the int4 range, including the i32 minimum endpoint.
    fn parse_json_index(&mut self) -> Result<i32, String> {
        let negative = self.consume("-");
        self.skip_space();
        let start = self.position;
        while self.peek_char().is_some_and(|ch| ch.is_ascii_digit()) {
            self.position += 1;
        }
        if start == self.position {
            return Err(self.unsupported_at("expected a signed decimal JSON array index"));
        }
        let magnitude = self.source[start..self.position]
            .parse::<u64>()
            .map_err(|_| "PostgreSQL JSON array index is outside the supported int4 range")?;
        // PostgreSQL resolves the signed lower endpoint as int4 too. Bound
        // the magnitude before converting, then negate in i64 so i32::MIN
        // never overflows during validation.
        let maximum = i32::MAX as u64 + u64::from(negative);
        if magnitude > maximum {
            return Err(
                "PostgreSQL JSON array indexes must fit the supported signed int4 range".into(),
            );
        }
        let magnitude = magnitude as i64;
        Ok((if negative { -magnitude } else { magnitude }) as i32)
    }

    /// Parse a primary expression and its postfix casts; Portable mode rejects PostgreSQL casts.
    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expression = self.parse_atom()?;
        loop {
            self.skip_space();
            if !self.source[self.position..].starts_with("::") {
                return Ok(expression);
            }
            if self.dialect != GeneratedExpressionDialect::Postgres {
                return Err(self.unsupported_at("PostgreSQL casts are unsupported"));
            }
            self.position += 2;
            self.skip_space();
            expression = match self.parse_cast_type()? {
                CastType::Text => Expr::TextCast(Box::new(expression)),
                CastType::Int4 => Expr::Int4Cast(Box::new(expression)),
            };
        }
    }

    /// Parse a string, column, parenthesized expression, `coalesce`, or enabled PostgreSQL `CAST` primary.
    fn parse_atom(&mut self) -> Result<Expr, String> {
        self.skip_space();
        match self.peek_char() {
            Some('(') => {
                self.position += 1;
                let expression = self.parse_operator_chain()?;
                self.skip_space();
                if !self.consume(")") {
                    return Err(self.unsupported_at("expected `)`"));
                }
                Ok(expression)
            }
            Some('\'') => self.read_string().map(Expr::String),
            Some('"') => self
                .read_quoted_identifier()
                .map(|name| Expr::Column { name, quoted: true }),
            Some(ch) if is_ident_start(ch) => {
                let name = self.read_identifier();
                self.skip_space();
                if self.peek_char() == Some('(') {
                    if name.eq_ignore_ascii_case("cast")
                        && self.dialect == GeneratedExpressionDialect::Postgres
                    {
                        self.position += 1;
                        return self.parse_cast_function();
                    }
                    if !name.eq_ignore_ascii_case("coalesce") {
                        return Err(format!(
                            "generated expression function `{name}` is unsupported"
                        ));
                    }
                    self.position += 1;
                    self.parse_coalesce()
                } else {
                    if is_sql_keyword(&name) {
                        return Err(format!(
                            "unquoted SQL keyword `{name}` is unsupported in generated expressions"
                        ));
                    }
                    Ok(Expr::Column {
                        name,
                        quoted: false,
                    })
                }
            }
            _ => Err(self.unsupported_at(
                "expected a text column, string literal, or parenthesized expression",
            )),
        }
    }

    /// Parse CAST(expr AS target) while preserving source SQL and enforcing the supported text or int4 target set.
    fn parse_cast_function(&mut self) -> Result<Expr, String> {
        let expression = self.parse_operator_chain()?;
        self.skip_space();
        if !self.consume_keyword("as") {
            return Err(self.unsupported_at("expected `AS` in PostgreSQL CAST"));
        }
        self.skip_space();
        let cast_type = self.parse_cast_type()?;
        self.skip_space();
        if !self.consume(")") {
            return Err(self.unsupported_at("expected `)` after PostgreSQL CAST"));
        }
        Ok(match cast_type {
            CastType::Text => Expr::TextCast(Box::new(expression)),
            CastType::Int4 => Expr::Int4Cast(Box::new(expression)),
        })
    }

    /// Accept only unmodified text, varchar, character varying, integer, or int4 targets; type modifiers remain unsupported.
    fn parse_cast_type(&mut self) -> Result<CastType, String> {
        let Some(ch) = self.peek_char() else {
            return Err(self.unsupported_at("expected a supported PostgreSQL cast target"));
        };
        if !is_ident_start(ch) {
            return Err(self.unsupported_at("expected a supported PostgreSQL cast target"));
        }
        let first = self.read_identifier();
        match first.to_ascii_lowercase().as_str() {
            "text" | "varchar" => {}
            "character" => {
                self.skip_space();
                if !self.consume_keyword("varying") {
                    return Err(
                        "only unbounded text, varchar, or character varying casts are supported in PostgreSQL generated expressions".into(),
                    );
                }
            }
            "integer" | "int4" => {
                self.skip_space();
                if self.peek_char() == Some('(') {
                    return Err(
                        "type-modified casts are unsupported in PostgreSQL generated expressions".into(),
                    );
                }
                return Ok(CastType::Int4);
            }
            _ => {
                return Err(format!(
                    "PostgreSQL generated-expression cast target `{first}` is unsupported; only unbounded text, varchar, character varying, integer, and int4 are supported"
                ));
            }
        }
        self.skip_space();
        if self.peek_char() == Some('(') {
            return Err("type-modified casts are unsupported in PostgreSQL generated expressions".into());
        }
        Ok(CastType::Text)
    }

    fn parse_coalesce(&mut self) -> Result<Expr, String> {
        let mut args = Vec::new();
        loop {
            args.push(self.parse_operator_chain()?);
            self.skip_space();
            if self.consume(",") {
                continue;
            }
            if self.consume(")") {
                break;
            }
            return Err(self.unsupported_at("expected `,` or `)` in coalesce"));
        }
        if args.len() < 2 {
            return Err(
                "coalesce in a generated expression requires at least two arguments".into(),
            );
        }
        Ok(Expr::Coalesce(args))
    }

    /// Return decoded SQL literal contents, collapsing doubled apostrophes and rejecting backslash escapes.
    fn read_string(&mut self) -> Result<String, String> {
        self.position += 1; // opening apostrophe
        let mut value = String::new();
        loop {
            let Some(ch) = self.peek_char() else {
                return Err("unterminated string literal in generated expression".into());
            };
            self.position += ch.len_utf8();
            match ch {
                '\'' if self.peek_char() == Some('\'') => {
                    self.position += 1;
                    value.push('\'');
                }
                '\'' => return Ok(value),
                '\\' => {
                    return Err(
                        "backslash escapes in generated string literals are unsupported".into(),
                    );
                }
                _ => value.push(ch),
            }
        }
    }

    fn read_quoted_identifier(&mut self) -> Result<String, String> {
        self.position += 1; // opening double quote
        let mut name = String::new();
        loop {
            let Some(ch) = self.peek_char() else {
                return Err("unterminated quoted identifier in generated expression".into());
            };
            self.position += ch.len_utf8();
            match ch {
                '"' if self.peek_char() == Some('"') => {
                    self.position += 1;
                    name.push('"');
                }
                '"' => {
                    if name.is_empty() {
                        return Err(
                            "empty quoted identifiers are unsupported in generated expressions"
                                .into(),
                        );
                    }
                    return Ok(name);
                }
                _ => name.push(ch),
            }
        }
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        while let Some(ch) = self.peek_char() {
            if !is_ident_continue(ch) {
                break;
            }
            self.position += ch.len_utf8();
        }
        self.source[start..self.position].to_string()
    }

    fn skip_space(&mut self) {
        while let Some(ch) = self.peek_char() {
            if !matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{000c}') {
                break;
            }
            self.position += ch.len_utf8();
        }
    }

    fn consume(&mut self, literal: &str) -> bool {
        if self.source[self.position..].starts_with(literal) {
            self.position += literal.len();
            true
        } else {
            false
        }
    }

    /// Match a SQL keyword without case sensitivity while requiring an identifier boundary.
    fn consume_keyword(&mut self, keyword: &str) -> bool {
        let remaining = &self.source[self.position..];
        let Some(prefix) = remaining.get(..keyword.len()) else {
            return false;
        };
        if !prefix.eq_ignore_ascii_case(keyword)
            || remaining[keyword.len()..].chars().next().is_some_and(is_ident_continue)
        {
            return false;
        }
        self.position += keyword.len();
        true
    }

    fn peek_char(&self) -> Option<char> {
        self.source.get(self.position..)?.chars().next()
    }

    fn unsupported_at(&self, message: &str) -> String {
        format!("{message} at byte {}", self.position)
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

/// PostgreSQL 18 keyword inventory from the release-pinned parser table:
/// https://github.com/postgres/postgres/blob/REL_18_0/src/include/parser/kwlist.h
/// PostgreSQL's versioned classification is documented at:
/// https://www.postgresql.org/docs/18/sql-keywords-appendix.html
/// This deliberately includes every keyword class (not only reserved words),
/// so a bare column reference must be quoted whenever either target treats
/// it as a keyword. That avoids context-dependent PostgreSQL token handling.
const POSTGRES_18_KEYWORDS: &str = "
abort absent absolute access action add admin after aggregate all
also alter always analyse analyze and any array as asc
asensitive assertion assignment asymmetric at atomic attach attribute authorization backward
before begin between bigint binary bit boolean both breadth by
cache call called cascade cascaded case cast catalog chain char
character characteristics check checkpoint class close cluster coalesce collate collation
column columns comment comments commit committed compression concurrently conditional configuration
conflict connection constraint constraints content continue conversion copy cost create
cross csv cube current current_catalog current_date current_role current_schema current_time current_timestamp
current_user cursor cycle data database day deallocate dec decimal declare
default defaults deferrable deferred definer delete delimiter delimiters depends depth
desc detach dictionary disable discard distinct do document domain double
drop each else empty enable encoding encrypted end enforced enum
error escape event except exclude excluding exclusive execute exists explain
expression extension external extract false family fetch filter finalize first
float following for force foreign format forward freeze from full
function functions generated global grant granted greatest group grouping groups
handler having header hold hour identity if ilike immediate immutable
implicit import in include including increment indent index indexes inherit
inherits initially inline inner inout input insensitive insert instead int
integer intersect interval into invoker is isnull isolation join json
json_array json_arrayagg json_exists json_object json_objectagg json_query json_scalar json_serialize json_table json_value
keep key keys label language large last lateral leading leakproof
least left level like limit listen load local localtime localtimestamp
location lock locked logged mapping match matched materialized maxvalue merge
merge_action method minute minvalue mode month move name names national
natural nchar nested new next nfc nfd nfkc nfkd no
none normalize normalized not nothing notify notnull nowait null nullif
nulls numeric object objects of off offset oids old omit
on only operator option options or order ordinality others out
outer over overlaps overlay overriding owned owner parallel parameter parser
partial partition passing password path period placing plan plans policy
position preceding precision prepare prepared preserve primary prior privileges procedural
procedure procedures program publication quote quotes range read real reassign
recursive ref references referencing refresh reindex relative release rename repeatable
replace replica reset restart restrict return returning returns revoke right
role rollback rollup routine routines row rows rule savepoint scalar
schema schemas scroll search second security select sequence sequences serializable
server session session_user set setof sets share show similar simple
skip smallint snapshot some source sql stable standalone start statement
statistics stdin stdout storage stored strict string strip subscription substring
support symmetric sysid system system_user table tables tablesample tablespace target
temp template temporary text then ties time timestamp to trailing
transaction transform treat trigger trim true truncate trusted type types
uescape unbounded uncommitted unconditional unencrypted union unique unknown unlisten unlogged
until update user using vacuum valid validate validator value values
varchar variadic varying verbose version view views virtual volatile when
where whitespace window with within without work wrapper write xml
xmlattributes xmlconcat xmlelement xmlexists xmlforest xmlnamespaces xmlparse xmlpi xmlroot xmlserialize
xmltable year yes zone
";

/// SQLite's full keyword inventory, including words omitted by builds with
/// optional features disabled, from:
/// https://www.sqlite.org/lang_keywords.html (147-word list, retrieved 2026-10-08).
const SQLITE_KEYWORDS: &str = "
ABORT ACTION ADD AFTER ALL ALTER ALWAYS ANALYZE AND AS
ASC ATTACH AUTOINCREMENT BEFORE BEGIN BETWEEN BY CASCADE CASE CAST
CHECK COLLATE COLUMN COMMIT CONFLICT CONSTRAINT CREATE CROSS CURRENT CURRENT_DATE
CURRENT_TIME CURRENT_TIMESTAMP DATABASE DEFAULT DEFERRABLE DEFERRED DELETE DESC DETACH DISTINCT
DO DROP EACH ELSE END ESCAPE EXCEPT EXCLUDE EXCLUSIVE EXISTS
EXPLAIN FAIL FILTER FIRST FOLLOWING FOR FOREIGN FROM FULL GENERATED
GLOB GROUP GROUPS HAVING IF IGNORE IMMEDIATE IN INDEX INDEXED
INITIALLY INNER INSERT INSTEAD INTERSECT INTO IS ISNULL JOIN KEY
LAST LEFT LIKE LIMIT MATCH MATERIALIZED NATURAL NO NOT NOTHING
NOTNULL NULL NULLS OF OFFSET ON OR ORDER OTHERS OUTER
OVER PARTITION PLAN PRAGMA PRECEDING PRIMARY QUERY RAISE RANGE RECURSIVE
REFERENCES REGEXP REINDEX RELEASE RENAME REPLACE RESTRICT RETURNING RIGHT ROLLBACK
ROW ROWS SAVEPOINT SELECT SET TABLE TEMP TEMPORARY THEN TIES
TO TRANSACTION TRIGGER UNBOUNDED UNION UNIQUE UPDATE USING VACUUM VALUES
VIEW VIRTUAL WHEN WHERE WINDOW WITH WITHOUT
";

/// A bare identifier that is a keyword in either supported DDL dialect must
/// be double-quoted in the retained source expression. We do not rewrite the
/// SQL text, so quoting remains an explicit source-level requirement.
fn is_sql_keyword(name: &str) -> bool {
    POSTGRES_18_KEYWORDS
        .split_ascii_whitespace()
        .chain(SQLITE_KEYWORDS.split_ascii_whitespace())
        .any(|keyword| name.eq_ignore_ascii_case(keyword))
}

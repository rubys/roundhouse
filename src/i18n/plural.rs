//! A locale's own plural rule, as the runtime evaluates it: prefix text
//! over the count's operands. A key is `:form` or `? <bool> <key> <key>`;
//! a bool is `& | !` over bools, `num` (the count is Numeric), `int x`,
//! `== != >= < x y`, `in x lo hi`, `inz x lo hi` (an integer in the
//! range, as `(lo..hi).to_a.include?` answers), or `of x k a1 … ak`; a
//! number is `% x y`, `floor x`, an operand or a literal. The operands
//! are rails-i18n's `r` (the count) and `d` (a fraction's digits), and
//! CLDR's `n i v f t`. rails-i18n's rules are in [`super::rails_i18n`];
//! this module reads an app's ruby-cldr `plurals.rb`.

use crate::ingest::prism_api::PrismLocation;
/// Each locale's rule in a ruby-cldr `plurals.rb` (`{ af: { i18n: {
/// plural: { rule: lambda … } } } }`), `None` for one whose lambda is
/// not the operands-and-comparisons shape ruby-cldr generates.
pub fn cldr_rules(source: &str) -> Vec<(String, Option<String>)> {
    let result = ruby_prism::parse(source.as_bytes());
    let root = result.node();
    let Some(program) = root.as_program_node() else { return Vec::new() };
    let Some(hash) = program.statements().body().iter().last().and_then(|n| n.as_hash_node()) else { return Vec::new() };
    let mut out = Vec::new();
    for element in hash.elements().iter() {
        let Some(assoc) = element.as_assoc_node() else { continue };
        let Some(locale) = symbol(&assoc.key()) else { continue };
        let rule = find_rule(&assoc.value()).and_then(|lambda| Cldr { src: source }.lambda(&lambda));
        out.push((locale, rule));
    }
    out
}

fn symbol(node: &ruby_prism::Node<'_>) -> Option<String> {
    node.as_symbol_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned())
}

fn find_rule<'pr>(node: &ruby_prism::Node<'pr>) -> Option<ruby_prism::Node<'pr>> {
    for element in node.as_hash_node()?.elements().iter() {
        let assoc = element.as_assoc_node()?;
        let value = assoc.value();
        if symbol(&assoc.key()).as_deref() == Some("rule") {
            return Some(value);
        }
        if let Some(found) = find_rule(&value) {
            return Some(found);
        }
    }
    None
}

struct Cldr<'s> {
    src: &'s str,
}

impl Cldr<'_> {
    /// The source with its whitespace dropped, for ruby-cldr's fixed idioms.
    fn squeezed(&self, node: &ruby_prism::Node<'_>) -> String {
        let loc = node.location();
        self.src[loc.start_offset()..loc.end_offset()].chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn lambda(&self, node: &ruby_prism::Node<'_>) -> Option<String> {
        let body = match node.as_lambda_node() {
            Some(lambda) => lambda.body()?,
            None => {
                let call = node.as_call_node().filter(|c| c.name().as_slice() == b"lambda" && c.receiver().is_none())?;
                call.block()?.as_block_node()?.body()?
            }
        };
        let statements: Vec<ruby_prism::Node<'_>> = body.as_statements_node()?.body().iter().collect();
        let rule = match statements.as_slice() {
            [prelude, rule] if self.squeezed(prelude) == r#"n=n.respond_to?(:abs)?n.abs:((m=n.to_s)[0]=="-"?m[1,m.length]:m)"# => rule,
            [rule] => rule,
            _ => return None,
        };
        self.key(rule)
    }

    fn single<'pr>(&self, statements: Option<ruby_prism::StatementsNode<'pr>>) -> Option<ruby_prism::Node<'pr>> {
        let body: Vec<ruby_prism::Node<'pr>> = statements?.body().iter().collect();
        match <[_; 1]>::try_from(body) {
            Ok([one]) => Some(one),
            Err(_) => None,
        }
    }

    fn key(&self, node: &ruby_prism::Node<'_>) -> Option<String> {
        if let Some(name) = symbol(node) {
            return Some(format!(":{name}"));
        }
        if let Some(paren) = node.as_parentheses_node() {
            return self.key(&self.single(paren.body()?.as_statements_node())?);
        }
        if let Some(branch) = node.as_if_node() {
            let cond = self.bool(&branch.predicate())?;
            let yes = self.key(&self.single(branch.statements())?)?;
            let no = match branch.subsequent()? {
                n if n.as_else_node().is_some() => self.key(&self.single(n.as_else_node()?.statements())?)?,
                n => self.key(&n)?,
            };
            return Some(format!("? {cond} {yes} {no}"));
        }
        None
    }

    fn bool(&self, node: &ruby_prism::Node<'_>) -> Option<String> {
        if let Some(paren) = node.as_parentheses_node() {
            return self.bool(&self.single(paren.body()?.as_statements_node())?);
        }
        if let Some(and) = node.as_and_node() {
            return Some(format!("& {} {}", self.bool(&and.left())?, self.bool(&and.right())?));
        }
        if let Some(or) = node.as_or_node() {
            return Some(format!("| {} {}", self.bool(&or.left())?, self.bool(&or.right())?));
        }
        let call = node.as_call_node()?;
        let name = String::from_utf8_lossy(call.name().as_slice()).into_owned();
        let recv = call.receiver()?;
        let args: Vec<ruby_prism::Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        match (name.as_str(), args.as_slice()) {
            ("!", []) => Some(format!("! {}", self.bool(&recv)?)),
            ("zero?", []) => Some(format!("== {} 0", self.num(&recv)?)),
            ("==" | "!=" | ">=" | "<", [arg]) => Some(format!("{name} {} {}", self.num(&recv)?, self.num(arg)?)),
            (">", [arg]) => Some(format!("< {} {}", self.num(arg)?, self.num(&recv)?)),
            ("<=", [arg]) => Some(format!(">= {} {}", self.num(arg)?, self.num(&recv)?)),
            ("include?" | "member?" | "cover?", [arg]) => {
                let x = self.num(arg)?;
                let set = match recv.as_parentheses_node() {
                    Some(paren) => self.single(paren.body()?.as_statements_node())?,
                    None => recv,
                };
                if let Some(range) = set.as_range_node() {
                    if range.is_exclude_end() {
                        return None;
                    }
                    return Some(format!("in {x} {} {}", self.literal(&range.left()?)?, self.literal(&range.right()?)?));
                }
                let items: Vec<String> = set.as_array_node()?.elements().iter().map(|e| self.literal(&e)).collect::<Option<_>>()?;
                Some(format!("of {x} {} {}", items.len(), items.join(" ")))
            }
            _ => None,
        }
    }

    fn literal(&self, node: &ruby_prism::Node<'_>) -> Option<String> {
        (node.as_integer_node().is_some() || node.as_float_node().is_some()).then(|| self.squeezed(node).replace('_', ""))
    }

    fn num(&self, node: &ruby_prism::Node<'_>) -> Option<String> {
        if let Some(literal) = self.literal(node) {
            return Some(literal);
        }
        match self.squeezed(node).as_str() {
            "n.to_f" => return Some("n".into()),
            "n.to_i" => return Some("i".into()),
            r#"((v=n.to_s.split(".")[1])?v.length:0)"# => return Some("v".into()),
            r#"((f=n.to_s.split(".")[1])?f.to_i:0)"# => return Some("f".into()),
            r#"((t=n.to_s.split(".")[1])?t.gsub(/0+$/,"").to_i:0)"# => return Some("t".into()),
            // ruby-cldr spells CLDR's compact-exponent operand as a constant 0.
            "(e=0)" | "e=0" => return Some("0".into()),
            _ => {}
        }
        if let Some(paren) = node.as_parentheses_node() {
            return self.num(&self.single(paren.body()?.as_statements_node())?);
        }
        let call = node.as_call_node()?;
        let args: Vec<ruby_prism::Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        match (call.name().as_slice(), args.as_slice()) {
            (b"%", [arg]) => Some(format!("% {} {}", self.num(&call.receiver()?)?, self.num(arg)?)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_cldr_lambdas_read_as_rules() {
        let source = r#"# generated by https://github.com/ruby-i18n/ruby-cldr
{
  pl: {
    i18n: {
      plural: {
        keys: %i[one few many other],
        rule:
          lambda do |n|
            n = n.respond_to?(:abs) ? n.abs : ((m = n.to_s)[0] == "-" ? m[1, m.length] : m)
            (n.to_i == 1 && ((v = n.to_s.split(".")[1]) ? v.length : 0) == 0) ? :one : ( ((v = n.to_s.split(".")[1]) ? v.length : 0) == 0 && (2..4).include?(n.to_i % 10) && !(12..14).include?(n.to_i % 100) ) ? :few : :other
          end,
      },
    },
  },
  ja: { i18n: { plural: { keys: %i[other], rule: lambda { |n| :other } } } },
  xx: { i18n: { plural: { keys: %i[other], rule: lambda { |n| n.foo ? :one : :other } } } },
}
"#;
        assert_eq!(
            cldr_rules(source),
            [
                ("pl".to_string(), Some("? & == i 1 == v 0 :one ? & & == v 0 in % i 10 2 4 ! in % i 100 12 14 :few :other".to_string())),
                ("ja".to_string(), Some(":other".to_string())),
                ("xx".to_string(), None),
            ]
        );
    }
}

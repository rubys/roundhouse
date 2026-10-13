#include "ruby.h"

static VALUE observer;
static VALUE attribute_backends;
static ID collector_id, addition_collector_id, failures_id, unproven_attributes_id, call_id, context_id;
static ID owner_id, super_method_id, unbind_id, bind_id, instance_method_id;
static ID method_added_id;

static VALUE notify(VALUE args)
{
    const VALUE *values = RARRAY_CONST_PTR(args);
    return rb_funcallv(values[0], call_id, RARRAY_LEN(args) - 1, values + 1);
}

static void notify_safely(VALUE args, VALUE owner, VALUE name, VALUE (*function)(VALUE))
{
    VALUE previous_error = rb_errinfo();
    int state = 0;
    rb_protect(function, args, &state);
    if (state) {
        VALUE error = rb_errinfo();
        if (!RB_TYPE_P(error, T_OBJECT) || !rb_obj_is_kind_of(error, rb_eStandardError))
            rb_jump_tag(state);
        rb_set_errinfo(previous_error);
        rb_ary_push(rb_ivar_get(observer, failures_id),
                    rb_ary_new_from_args(3, owner, name, error));
    }
    RB_GC_GUARD(args);
    RB_GC_GUARD(previous_error);
}

static VALUE instance_definition(VALUE args)
{
    return rb_funcall(rb_ary_entry(args, 1), instance_method_id, 1, rb_ary_entry(args, 2));
}

static VALUE missing_definition(VALUE args, VALUE error)
{
    return Qnil;
}

static VALUE capture_addition(VALUE args)
{
    VALUE self = rb_ary_entry(args, 1), name = rb_ary_entry(args, 2);
    VALUE context = rb_thread_local_aref(rb_thread_current(), context_id);
    VALUE kind = Qnil;
    if (!NIL_P(context) && rb_ary_entry(context, 0) == self &&
        RTEST(rb_ary_entry(context, 2)) &&
        rb_funcall(rb_obj_method(self, ID2SYM(method_added_id)), owner_id, 0) == observer) {
        kind = rb_hash_aref(rb_ary_entry(context, 1), name);
    }
    /* Save the installed entry before super. This precedes the application
     * method_added chain, NOT arbitrary application TracePoints. The collector
     * must additionally verify this saved body's native accessor semantics. */
    VALUE definition = rb_rescue2(instance_definition, args, missing_definition, Qnil, rb_eNameError, (VALUE)0);
    rb_ary_store(args, 3, definition);
    rb_ary_store(args, 4, kind);
    return notify(args);
}

static VALUE observe_method_added(VALUE self, VALUE name)
{
    VALUE collector = rb_ivar_get(observer, addition_collector_id);
    if (!NIL_P(collector))
        notify_safely(rb_ary_new_from_args(5, collector, self, name, Qnil, Qnil), self, name, capture_addition);
    RB_GC_GUARD(collector);
    return rb_call_super(1, &name);
}

static VALUE call_attribute(VALUE args)
{
    VALUE argv = rb_ary_entry(args, 0);
    VALUE result = rb_call_super_kw(RARRAY_LEN(argv), RARRAY_CONST_PTR(argv), RB_PASS_CALLED_KEYWORDS);
    rb_ary_store(args, 3, Qtrue);
    return result;
}

static VALUE finish_attribute(VALUE args)
{
    VALUE thread = rb_thread_current();
    rb_thread_local_aset(thread, context_id, rb_ary_entry(args, 1));
    VALUE context = rb_ary_entry(args, 2);
    VALUE self = rb_ary_entry(context, 0);
    VALUE names = rb_ary_entry(context, 3);
    if (!RTEST(rb_ary_entry(args, 3))) {
        /* Never invoke a collector while unwinding: a second throw/exception
         * could replace the first tag/payload. Retain only a refusal ledger. */
        rb_ary_push(rb_ivar_get(observer, unproven_attributes_id), rb_ary_new_from_args(2, self, names));
        return Qnil;
    }
    VALUE collector = rb_ivar_get(observer, addition_collector_id);
    if (!NIL_P(collector)) {
        notify_safely(rb_ary_new_from_args(5, collector, self, names, Qnil,
                      ID2SYM(rb_intern("attribute_end"))), self, names, notify);
    }
    return Qnil;
}

static VALUE attribute_proof(VALUE args)
{
    VALUE self = rb_ary_entry(args, 0), operation = rb_ary_entry(args, 1);
    VALUE callback = rb_obj_method(self, ID2SYM(method_added_id));
    VALUE method = rb_obj_method(self, operation);
    while (!NIL_P(method) && rb_funcall(method, owner_id, 0) != observer)
        method = rb_funcall(method, super_method_id, 0);
    VALUE backend = NIL_P(method) ? Qnil : rb_funcall(method, super_method_id, 0);
    VALUE original = rb_funcall(rb_hash_aref(attribute_backends, operation), bind_id, 1, self);
    return rb_funcall(callback, owner_id, 0) == observer && !NIL_P(backend) &&
        RTEST(rb_equal(rb_funcall(backend, unbind_id, 0), rb_funcall(original, unbind_id, 0))) ? Qtrue : Qfalse;
}

static VALUE observe_attribute(int argc, VALUE *argv, VALUE self, ID operation)
{
    if (NIL_P(rb_ivar_get(observer, addition_collector_id)))
        return rb_call_super_kw(argc, argv, RB_PASS_CALLED_KEYWORDS);

    VALUE names = rb_hash_new();
    VALUE installed_names = rb_ary_new();
    int symbols_only = 1;
    for (int i = 0; i < argc; i++) {
        if (!SYMBOL_P(argv[i])) {
            symbols_only = 0;
            continue;
        }
        ID name = SYM2ID(argv[i]);
        if (operation != rb_intern("attr_writer")) {
            rb_hash_aset(names, argv[i], ID2SYM(rb_intern("attr_reader")));
            rb_ary_push(installed_names, argv[i]);
        }
        if (operation != rb_intern("attr_reader")) {
            VALUE writer = ID2SYM(rb_id_attrset(name));
            rb_hash_aset(names, writer, ID2SYM(rb_intern("attr_writer")));
            rb_ary_push(installed_names, writer);
        }
    }
    /* Undefining method_added is legal. It disables proof, not the original
     * operation. No name coercion or installation is performed by this query. */
    VALUE proof_args = rb_ary_new_from_args(2, self, ID2SYM(operation));
    VALUE proof = symbols_only ? rb_rescue2(attribute_proof, proof_args, missing_definition, Qnil,
                                           rb_eNameError, (VALUE)0) : Qfalse;

    VALUE context = rb_ary_new_from_args(4, self, names, proof, installed_names);
    VALUE thread = rb_thread_current();
    VALUE args = rb_ary_new_from_args(4, rb_ary_new_from_values(argc, argv),
                                    rb_thread_local_aref(thread, context_id), context, Qfalse);
    rb_thread_local_aset(thread, context_id, context);
    /* Native coercion, visibility, argument/block forwarding and unwind remain
     * MRI's. Non-Symbol inputs are never coerced by this observer. */
    VALUE result = rb_ensure(call_attribute, args, finish_attribute, args);
    RB_GC_GUARD(args);
    RB_GC_GUARD(proof_args);
    return result;
}

static VALUE observe_attr_reader(int argc, VALUE *argv, VALUE self)
{
    return observe_attribute(argc, argv, self, rb_intern("attr_reader"));
}

static VALUE observe_attr_writer(int argc, VALUE *argv, VALUE self)
{
    return observe_attribute(argc, argv, self, rb_intern("attr_writer"));
}

static VALUE observe_attr_accessor(int argc, VALUE *argv, VALUE self)
{
    return observe_attribute(argc, argv, self, rb_intern("attr_accessor"));
}

static VALUE observe_define_method(int argc, VALUE *argv, VALUE self)
{
    VALUE collector = rb_ivar_get(observer, collector_id);
    VALUE callable = Qnil;
    if (!NIL_P(collector)) {
        /* MRI ignores a passed block when an explicit body is supplied.
         * Do not coerce names, validate arguments or reflect through Ruby here. */
        if (argc == 2) callable = argv[1];
        else if (argc == 1 && rb_block_given_p()) callable = rb_block_proc();
    }

    /* No rescue, Ruby frame, visibility repair or substituted block/arguments. */
    VALUE result = rb_call_super_kw(argc, argv, RB_PASS_CALLED_KEYWORDS);
    if (!NIL_P(collector)) {
        VALUE args = rb_ary_new_from_args(4, collector, self, result, callable);
        notify_safely(args, self, result, notify);
        RB_GC_GUARD(args);
    }
    RB_GC_GUARD(collector);
    RB_GC_GUARD(callable);
    return result;
}

void Init_native_define_method_observer(void)
{
    observer = rb_define_module("NativeDefineMethodObserver");
    rb_global_variable(&observer);
    collector_id = rb_intern("@collector");
    addition_collector_id = rb_intern("@addition_collector");
    failures_id = rb_intern("@failures");
    unproven_attributes_id = rb_intern("@unproven_attributes");
    call_id = rb_intern("call");
    context_id = rb_intern("__native_attribute_observer_context__");
    owner_id = rb_intern("owner");
    super_method_id = rb_intern("super_method");
    unbind_id = rb_intern("unbind");
    bind_id = rb_intern("bind");
    instance_method_id = rb_intern("instance_method");
    method_added_id = rb_intern("method_added");
    attribute_backends = rb_hash_new();
    rb_global_variable(&attribute_backends);
    const char *attributes[] = {"attr_reader", "attr_writer", "attr_accessor"};
    for (int i = 0; i < 3; i++) {
        VALUE name = ID2SYM(rb_intern(attributes[i]));
        rb_hash_aset(attribute_backends, name, rb_funcall(rb_cModule, instance_method_id, 1, name));
    }
    rb_define_attr(rb_singleton_class(observer), "collector", 1, 1);
    rb_define_attr(rb_singleton_class(observer), "addition_collector", 1, 1);
    rb_define_attr(rb_singleton_class(observer), "failures", 1, 0);
    rb_define_attr(rb_singleton_class(observer), "unproven_attributes", 1, 0);
    rb_ivar_set(observer, failures_id, rb_ary_new());
    rb_ivar_set(observer, unproven_attributes_id, rb_ary_new());
    rb_define_method(observer, "define_method", observe_define_method, -1);
    rb_define_private_method(observer, "method_added", observe_method_added, 1);
    rb_define_method(observer, "attr_reader", observe_attr_reader, -1);
    rb_define_method(observer, "attr_writer", observe_attr_writer, -1);
    rb_define_method(observer, "attr_accessor", observe_attr_accessor, -1);
    rb_prepend_module(rb_cModule, observer);
}

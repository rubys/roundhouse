#include "ruby.h"

static VALUE observer;
static ID collector_id, failures_id, call_id;

static VALUE notify(VALUE args)
{
    const VALUE *values = RARRAY_CONST_PTR(args);
    return rb_funcallv(values[0], call_id, 3, values + 1);
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
        VALUE previous_error = rb_errinfo();
        VALUE args = rb_ary_new_from_args(4, collector, self, result, callable);
        int state = 0;
        rb_protect(notify, args, &state);
        if (state) {
            VALUE error = rb_errinfo();
            /* Only ordinary exporter failures are deferred. Preserve process
             * cancellation, exit and nonlocal control flow with the original tag. */
            if (!RB_TYPE_P(error, T_OBJECT) || !rb_obj_is_kind_of(error, rb_eStandardError))
                rb_jump_tag(state);
            rb_set_errinfo(previous_error);
            rb_ary_push(rb_ivar_get(observer, failures_id),
                        rb_ary_new_from_args(3, self, result, error));
        }
        RB_GC_GUARD(args);
        RB_GC_GUARD(previous_error);
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
    failures_id = rb_intern("@failures");
    call_id = rb_intern("call");
    rb_define_attr(rb_singleton_class(observer), "collector", 1, 1);
    rb_define_attr(rb_singleton_class(observer), "failures", 1, 0);
    rb_ivar_set(observer, failures_id, rb_ary_new());
    rb_define_method(observer, "define_method", observe_define_method, -1);
    rb_prepend_module(rb_cModule, observer);
}

<script lang="ts">
  /*
   * A dialog's refusal, `lead` and then the daemon's `reason`, announced and focused as it appears: the
   * button that sent the request is still where the keyboard was, and would leave the refusal unread.
   */
  let { lead, reason }: { lead: string; reason: string } = $props()
</script>

<p class="dialog-text error" role="alert" tabindex="-1" {@attach (node) => node.focus()}>
  {lead}
  {reason}
</p>

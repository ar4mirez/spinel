# Clearing a full SizedQueue frees every slot, so every blocked pusher
# finishes, as CRuby 4.0.9's `rb_szqueue_clear` wakes them all.
q = SizedQueue.new(2)
q << 1
q << 2
pushers = [3, 4].map { |x| Thread.new { q.push(x) } }
Thread.pass until pushers.all?(&:stop?) && q.num_waiting == 2
q.clear
pushers.each(&:join)
p q.size
